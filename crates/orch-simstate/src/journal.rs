//! The crash-consistent append-only op journal (plan D-T2).
//!
//! One file per state-dir (`<dir>/journal.v1`), shared by all four service
//! wrappers behind a mutex. Frame format:
//!
//! ```text
//! u32 LE payload length | u64 LE truncated blake3 of payload | payload
//! ```
//!
//! payload = postcard-encoded [`JournalRecord`]. Op frames fsync
//! (`sync_data`) before the op applies to the in-memory fake — write-ahead.
//! `Applied` frames are advisory (no fsync; losing one is
//! "executed, response lost", exactly the crash semantics real clients
//! face). Torn tails are expected and truncated on load; mid-file
//! corruption is a loud panic.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use crate::records::JournalRecord;

pub const JOURNAL_FILE: &str = "journal.v1";
pub const JOURNAL_VERSION: u32 = 1;
const FRAME_HEADER_LEN: usize = 4 + 8;

/// Truncated blake3: first 8 bytes, little-endian.
#[must_use]
pub fn truncated_blake3(payload: &[u8]) -> u64 {
    let hash = blake3::hash(payload);
    u64::from_le_bytes(hash.as_bytes()[..8].try_into().expect("8 bytes"))
}

/// What kind of state the record being appended mutates, for the forced
/// torn-write hook (plan D-T3). Only `put_metadata` appends distinguish
/// WAL/checkpoint keys; everything else is `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordKind {
    WalAppend,
    CkptPut,
    Other,
}

impl RecordKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::WalAppend => "wal-append",
            Self::CkptPut => "ckpt-put",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoadStats {
    pub frames: u64,
    pub truncated_bytes: u64,
}

#[derive(Debug)]
struct TornHook {
    kind: RecordKind,
    nth: u32,
    seen: u32,
}

fn torn_hook_from_env() -> Option<TornHook> {
    let value = std::env::var("ORCH_SIM_TORN_AT").ok()?;
    let (kind, nth) = value.split_once(':')?;
    let kind = match kind {
        "wal-append" => RecordKind::WalAppend,
        "ckpt-put" => RecordKind::CkptPut,
        other => panic!("ORCH_SIM_TORN_AT: unknown kind '{other}'"),
    };
    let nth: u32 = nth
        .parse()
        .unwrap_or_else(|_| panic!("ORCH_SIM_TORN_AT: bad nth in '{value}'"));
    assert!(nth > 0, "ORCH_SIM_TORN_AT: nth must be >= 1");
    Some(TornHook { kind, nth, seen: 0 })
}

/// Append-only journal over `<dir>/journal.v1`.
pub struct Journal {
    file: File,
    next_op_id: u64,
    torn: Option<TornHook>,
}

impl Journal {
    /// Creates a fresh journal, writes the version header frame, and fsyncs
    /// both the file and the directory (a SIGKILL right after create must
    /// not lose the file itself).
    pub fn create(dir: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .append(true)
            .create_new(true)
            .open(dir.join(JOURNAL_FILE))?;
        let mut journal = Self {
            file,
            next_op_id: 1,
            torn: torn_hook_from_env(),
        };
        journal.write_frame(
            &JournalRecord::Header {
                version: JOURNAL_VERSION,
            },
            true,
        )?;
        File::open(dir)?.sync_data()?;
        Ok(journal)
    }

    /// Opens an existing journal for appending. `next_op_id` comes from the
    /// caller's [`Journal::load`] pass (max seen op id + 1).
    pub fn open_existing(dir: &Path, next_op_id: u64) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .append(true)
            .open(dir.join(JOURNAL_FILE))?;
        Ok(Self {
            file,
            next_op_id,
            torn: torn_hook_from_env(),
        })
    }

    /// Assigns the next op id, builds the record, appends it write-ahead
    /// (fsync), and returns the op id. Journal I/O failures are harness
    /// environment failures, not tested semantics — they panic.
    pub fn append_op(&mut self, build: impl FnOnce(u64) -> JournalRecord, kind: RecordKind) -> u64 {
        let op_id = self.next_op_id;
        self.next_op_id += 1;
        let record = build(op_id);
        self.maybe_tear(&record, kind);
        self.write_frame(&record, true).expect("journal append");
        op_id
    }

    /// Appends an advisory frame (`Applied`) with no fsync — losing it to a
    /// crash is fine by design (D-T2).
    pub fn append_advisory(&mut self, record: &JournalRecord) {
        self.write_frame(record, false).expect("journal append");
    }

    fn write_frame(&mut self, record: &JournalRecord, sync: bool) -> std::io::Result<()> {
        let payload = postcard::to_allocvec(record).expect("journal records are serializable");
        let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
        frame.extend_from_slice(
            &u32::try_from(payload.len())
                .expect("journal payload fits u32")
                .to_le_bytes(),
        );
        frame.extend_from_slice(&truncated_blake3(&payload).to_le_bytes());
        frame.extend_from_slice(&payload);
        self.file.write_all(&frame)?;
        if sync {
            self.file.sync_data()?;
        }
        Ok(())
    }

    /// The forced torn-write hook (plan D-T3): on the nth matching append,
    /// write a prefix of the frame (header + half the payload), sync, print
    /// the harness marker, and park forever so the harness lands a real
    /// SIGKILL exactly mid-append.
    fn maybe_tear(&mut self, record: &JournalRecord, kind: RecordKind) {
        let Some(torn) = &mut self.torn else { return };
        if kind != torn.kind {
            return;
        }
        torn.seen += 1;
        if torn.seen != torn.nth {
            return;
        }
        let payload = postcard::to_allocvec(record).expect("journal records are serializable");
        let mut prefix = Vec::with_capacity(FRAME_HEADER_LEN + payload.len() / 2);
        prefix.extend_from_slice(
            &u32::try_from(payload.len())
                .expect("journal payload fits u32")
                .to_le_bytes(),
        );
        prefix.extend_from_slice(&truncated_blake3(&payload).to_le_bytes());
        prefix.extend_from_slice(&payload[..payload.len() / 2]);
        self.file.write_all(&prefix).expect("torn prefix write");
        self.file.sync_data().expect("torn prefix sync");
        println!("TIER2_CHAOS_HANG kind={}", kind.as_str());
        std::io::stdout().flush().expect("stdout flush");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }

    /// Scans the journal, truncating a torn tail (`set_len` + sync) at the
    /// first short/corrupt trailing frame. A checksum failure *followed by*
    /// more valid frames is real corruption, not a torn tail — loud panic.
    /// An empty file (SIGKILL between create and the header write) loads
    /// empty; a nonempty journal must start with a version-1 header.
    pub fn load(dir: &Path) -> std::io::Result<(Vec<JournalRecord>, LoadStats)> {
        let path = dir.join(JOURNAL_FILE);
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        let mut bytes = Vec::new();
        (&file).read_to_end(&mut bytes)?;

        let mut records = Vec::new();
        let mut offset = 0usize;
        let mut torn_at: Option<usize> = None;
        while offset < bytes.len() {
            match parse_frame(&bytes, offset) {
                FrameParse::Ok { record, next } => {
                    records.push(*record);
                    offset = next;
                }
                FrameParse::Torn => {
                    torn_at = Some(offset);
                    break;
                }
                FrameParse::Corrupt { next } => {
                    // Torn tail or mid-file corruption? Decide by attempting
                    // to parse past the bad frame: any valid frame after it
                    // means the file kept growing after the damage.
                    let mut probe = next;
                    while probe < bytes.len() {
                        match parse_frame(&bytes, probe) {
                            FrameParse::Ok { .. } => panic!(
                                "journal {path:?}: corrupt frame at byte {offset} followed by \
                                 valid frames — mid-file corruption, not a torn tail"
                            ),
                            FrameParse::Corrupt { next } => probe = next,
                            FrameParse::Torn => break,
                        }
                    }
                    torn_at = Some(offset);
                    break;
                }
            }
        }

        let mut truncated_bytes = 0u64;
        if let Some(torn_at) = torn_at {
            truncated_bytes = (bytes.len() - torn_at) as u64;
            file.set_len(torn_at as u64)?;
            file.sync_data()?;
        }

        if let Some(first) = records.first() {
            assert_eq!(
                first,
                &JournalRecord::Header {
                    version: JOURNAL_VERSION
                },
                "journal {path:?}: unsupported or missing version header"
            );
        }

        let stats = LoadStats {
            frames: records.len() as u64,
            truncated_bytes,
        };
        Ok((records, stats))
    }
}

enum FrameParse {
    Ok {
        record: Box<JournalRecord>,
        next: usize,
    },
    /// Frame runs past EOF — can only be the (expected, torn) tail.
    Torn,
    /// Complete-length frame whose checksum or decode fails.
    Corrupt { next: usize },
}

fn parse_frame(bytes: &[u8], offset: usize) -> FrameParse {
    let remaining = &bytes[offset..];
    if remaining.len() < FRAME_HEADER_LEN {
        return FrameParse::Torn;
    }
    let len = u32::from_le_bytes(remaining[..4].try_into().expect("4 bytes")) as usize;
    let checksum = u64::from_le_bytes(remaining[4..12].try_into().expect("8 bytes"));
    if remaining.len() < FRAME_HEADER_LEN + len {
        return FrameParse::Torn;
    }
    let payload = &remaining[FRAME_HEADER_LEN..FRAME_HEADER_LEN + len];
    let next = offset + FRAME_HEADER_LEN + len;
    if truncated_blake3(payload) != checksum {
        return FrameParse::Corrupt { next };
    }
    match postcard::from_bytes::<JournalRecord>(payload) {
        Ok(record) => FrameParse::Ok {
            record: Box::new(record),
            next,
        },
        Err(_) => FrameParse::Corrupt { next },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_records(journal: &mut Journal) -> Vec<JournalRecord> {
        let mut written = Vec::new();
        for index in 0..4u64 {
            let op_id = journal.append_op(
                |op_id| JournalRecord::ReclaimSession { op_id },
                RecordKind::Other,
            );
            written.push(JournalRecord::ReclaimSession { op_id });
            let applied = JournalRecord::Applied {
                op_id,
                digest: 0xDEAD_BEEF ^ index,
            };
            journal.append_advisory(&applied);
            written.push(applied);
        }
        written
    }

    #[test]
    fn journal_round_trips_records() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut journal = Journal::create(dir.path()).expect("create");
        let written = sample_records(&mut journal);
        drop(journal);

        let (records, stats) = Journal::load(dir.path()).expect("load");
        assert_eq!(records[0], JournalRecord::Header { version: 1 });
        assert_eq!(&records[1..], written.as_slice());
        assert_eq!(stats.frames, 1 + written.len() as u64);
        assert_eq!(stats.truncated_bytes, 0);
    }

    #[test]
    fn torn_tail_at_every_offset_reloads_to_exactly_the_prefix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut journal = Journal::create(dir.path()).expect("create");
        let written = sample_records(&mut journal);
        drop(journal);
        let full = std::fs::read(dir.path().join(JOURNAL_FILE)).expect("read journal");

        // Find the last frame's start so every cut lands inside it.
        let last_payload = postcard::to_allocvec(written.last().expect("records")).expect("encode");
        let last_frame_len = FRAME_HEADER_LEN + last_payload.len();
        let last_start = full.len() - last_frame_len;

        for cut in 0..last_frame_len {
            let torn_dir = tempfile::tempdir().expect("tempdir");
            std::fs::write(
                torn_dir.path().join(JOURNAL_FILE),
                &full[..last_start + cut],
            )
            .expect("write torn copy");
            let (records, stats) = Journal::load(torn_dir.path()).expect("load torn");
            assert_eq!(&records[1..], &written[..written.len() - 1], "cut at {cut}");
            assert_eq!(stats.truncated_bytes, cut as u64, "cut at {cut}");
            // Reload after truncation is clean.
            let (again, again_stats) = Journal::load(torn_dir.path()).expect("reload");
            assert_eq!(again, records);
            assert_eq!(again_stats.truncated_bytes, 0);
        }
    }

    #[test]
    #[should_panic(expected = "mid-file corruption")]
    fn bit_flip_mid_file_panics() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut journal = Journal::create(dir.path()).expect("create");
        sample_records(&mut journal);
        drop(journal);

        let path = dir.path().join(JOURNAL_FILE);
        let mut bytes = std::fs::read(&path).expect("read journal");
        // Flip a payload byte in the second frame (first frame is the
        // header; keep it intact so the version check is not what fires).
        let header_payload =
            postcard::to_allocvec(&JournalRecord::Header { version: 1 }).expect("encode header");
        let flip_at = FRAME_HEADER_LEN + header_payload.len() + FRAME_HEADER_LEN;
        bytes[flip_at] ^= 0x40;
        std::fs::write(&path, &bytes).expect("write corrupted");

        let _ = Journal::load(dir.path());
    }

    #[test]
    fn empty_file_loads_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(JOURNAL_FILE), b"").expect("write empty");
        let (records, stats) = Journal::load(dir.path()).expect("load empty");
        assert!(records.is_empty());
        assert_eq!(stats, LoadStats::default());
    }

    #[test]
    #[should_panic(expected = "version header")]
    fn missing_header_panics() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let file = OpenOptions::new()
                .append(true)
                .create_new(true)
                .open(dir.path().join(JOURNAL_FILE))
                .expect("create raw");
            let mut journal = Journal {
                file,
                next_op_id: 1,
                torn: None,
            };
            journal.append_op(
                |op_id| JournalRecord::ReclaimSession { op_id },
                RecordKind::Other,
            );
        }
        let _ = Journal::load(dir.path());
    }
}
