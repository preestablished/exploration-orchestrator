//! Deterministic ChaCha12 substreams for reproducible search decisions.

use core::fmt;

use rand_chacha::ChaCha12Rng;
use rand_core::{Error as RandError, RngCore, SeedableRng};
use serde::{Deserialize, Serialize};

pub const STREAM_SEED_LEN: usize = 32;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RngPurpose {
    #[default]
    Selection,
    Synth,
    Misc,
    /// Guest entropy for the bootstrap `CreateVm` (API.md §2.1 step 1).
    Boot,
    /// Per-job guest entropy for expansion dispatch (API.md §2.2).
    Entropy,
}

impl RngPurpose {
    pub const fn tag(self) -> &'static [u8] {
        match self {
            Self::Selection => b"selection",
            Self::Synth => b"synth",
            Self::Misc => b"misc",
            Self::Boot => b"boot",
            Self::Entropy => b"entropy",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StreamSpec {
    pub seed: u64,
    pub purpose: RngPurpose,
    pub batch_seq: u64,
}

impl StreamSpec {
    pub const fn new(seed: u64, purpose: RngPurpose, batch_seq: u64) -> Self {
        Self {
            seed,
            purpose,
            batch_seq,
        }
    }
}

#[derive(Clone)]
pub struct DeterministicRng {
    spec: StreamSpec,
    inner: ChaCha12Rng,
    draws: u64,
}

impl DeterministicRng {
    pub fn from_spec(spec: StreamSpec) -> Self {
        let stream_seed = derive_stream_seed(spec.seed, spec.purpose, spec.batch_seq);
        Self {
            spec,
            inner: ChaCha12Rng::from_seed(stream_seed),
            draws: 0,
        }
    }

    pub fn new(seed: u64, purpose: RngPurpose, batch_seq: u64) -> Self {
        Self::from_spec(StreamSpec::new(seed, purpose, batch_seq))
    }

    pub fn selection(seed: u64, batch_seq: u64) -> Self {
        Self::new(seed, RngPurpose::Selection, batch_seq)
    }

    pub fn synth(seed: u64, batch_seq: u64) -> Self {
        Self::new(seed, RngPurpose::Synth, batch_seq)
    }

    pub fn misc(seed: u64, batch_seq: u64) -> Self {
        Self::new(seed, RngPurpose::Misc, batch_seq)
    }

    pub const fn spec(&self) -> StreamSpec {
        self.spec
    }

    pub const fn draw_count(&self) -> u64 {
        self.draws
    }

    pub fn next_unit_f64(&mut self) -> f64 {
        const SCALE: f64 = 1.0 / ((1u64 << 53) as f64);
        let bits = self.next_u64() >> 11;
        (bits as f64) * SCALE
    }

    fn bump_draws(&mut self) {
        self.draws = self
            .draws
            .checked_add(1)
            .expect("deterministic rng draw count overflowed");
    }
}

impl fmt::Debug for DeterministicRng {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeterministicRng")
            .field("spec", &self.spec)
            .field("draws", &self.draws)
            .finish_non_exhaustive()
    }
}

impl RngCore for DeterministicRng {
    fn next_u32(&mut self) -> u32 {
        let value = self.inner.next_u32();
        self.bump_draws();
        value
    }

    fn next_u64(&mut self) -> u64 {
        let value = self.inner.next_u64();
        self.bump_draws();
        value
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        rand_core::impls::fill_bytes_via_next(self, dest);
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), RandError> {
        self.fill_bytes(dest);
        Ok(())
    }
}

pub fn derive_stream_seed(seed: u64, purpose: RngPurpose, batch_seq: u64) -> [u8; STREAM_SEED_LEN] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&seed.to_le_bytes());
    hasher.update(purpose.tag());
    hasher.update(&batch_seq.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// Derives the only orchestrator seed used for one input-synth request.
///
/// The request seed is the first `next_u64()` draw from
/// [`DeterministicRng::synth`] for the experiment seed and expansion batch
/// sequence. Node ids are not part of this seed rule.
pub fn derive_synth_request_seed(experiment_seed: u64, batch_seq: u64) -> u64 {
    DeterministicRng::synth(experiment_seed, batch_seq).next_u64()
}

/// Derives the guest entropy seed for the bootstrap `CreateVm`.
///
/// One boot per experiment run: the stream is `("boot", batch_seq = 0)`.
pub fn derive_boot_entropy_seed(experiment_seed: u64) -> [u8; STREAM_SEED_LEN] {
    derive_stream_seed(experiment_seed, RngPurpose::Boot, 0)
}

/// Derives the guest entropy seed for one expansion job.
///
/// Per-batch stream `("entropy", batch_seq)`, then one blake3 fold per job
/// index so sibling jobs draw independent seeds without stream cursors.
pub fn derive_job_entropy_seed(
    experiment_seed: u64,
    batch_seq: u64,
    job_idx: u32,
) -> [u8; STREAM_SEED_LEN] {
    let stream_seed = derive_stream_seed(experiment_seed, RngPurpose::Entropy, batch_seq);
    let mut hasher = blake3::Hasher::new();
    hasher.update(&stream_seed);
    hasher.update(&job_idx.to_le_bytes());
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN_SEED: u64 = 0x0123_4567_89ab_cdef;

    const SELECTION_BATCH_0_SEED: [u8; STREAM_SEED_LEN] = [
        117, 126, 6, 111, 52, 234, 209, 119, 249, 33, 15, 235, 134, 66, 129, 3, 139, 56, 210, 79,
        169, 91, 203, 179, 162, 34, 8, 241, 16, 41, 186, 86,
    ];
    const SELECTION_BATCH_0_DRAWS: [u64; 3] = [
        16_182_809_055_178_078_652,
        12_453_579_497_275_307_868,
        3_484_137_952_289_349_336,
    ];

    const SYNTH_BATCH_7_SEED: [u8; STREAM_SEED_LEN] = [
        58, 148, 0, 168, 195, 107, 25, 55, 177, 228, 166, 247, 73, 23, 226, 192, 15, 75, 21, 123,
        11, 96, 150, 160, 112, 178, 238, 56, 42, 195, 229, 3,
    ];
    const SYNTH_BATCH_7_DRAWS: [u64; 3] = [
        8_371_989_289_210_138_313,
        13_901_018_766_997_971_418,
        11_842_641_017_646_055_190,
    ];

    const MISC_BATCH_42_SEED: [u8; STREAM_SEED_LEN] = [
        66, 19, 102, 166, 116, 165, 108, 38, 16, 176, 109, 156, 198, 100, 21, 93, 255, 236, 65, 71,
        198, 173, 244, 219, 185, 48, 9, 2, 195, 110, 209, 159,
    ];
    const MISC_BATCH_42_DRAWS: [u64; 3] = [
        13_912_786_952_531_663_816,
        17_923_498_092_330_976_264,
        13_527_300_642_075_294_954,
    ];

    #[test]
    fn rng_golden_vectors_are_checked_for_each_purpose() {
        for (purpose, batch_seq, expected_seed, expected_draws) in [
            (
                RngPurpose::Selection,
                0,
                SELECTION_BATCH_0_SEED,
                SELECTION_BATCH_0_DRAWS,
            ),
            (
                RngPurpose::Synth,
                7,
                SYNTH_BATCH_7_SEED,
                SYNTH_BATCH_7_DRAWS,
            ),
            (
                RngPurpose::Misc,
                42,
                MISC_BATCH_42_SEED,
                MISC_BATCH_42_DRAWS,
            ),
        ] {
            let mut rng = DeterministicRng::new(GOLDEN_SEED, purpose, batch_seq);

            assert_eq!(
                derive_stream_seed(GOLDEN_SEED, purpose, batch_seq),
                expected_seed
            );
            assert_eq!(rng.draw_count(), 0);
            assert_eq!(
                [rng.next_u64(), rng.next_u64(), rng.next_u64()],
                expected_draws
            );
            assert_eq!(rng.draw_count(), 3);
        }
    }

    #[test]
    fn synth_request_seed_uses_first_synth_stream_draw() {
        assert_eq!(
            derive_synth_request_seed(GOLDEN_SEED, 7),
            8_371_989_289_210_138_313
        );
    }

    #[test]
    fn rng_resume_reconstructs_same_stream_from_spec() {
        let spec = StreamSpec::new(987_654_321, RngPurpose::Synth, 19);
        let mut original = DeterministicRng::from_spec(spec);
        let mut resumed = DeterministicRng::from_spec(original.spec());
        let mut next_batch = DeterministicRng::from_spec(StreamSpec::new(
            spec.seed,
            spec.purpose,
            spec.batch_seq + 1,
        ));

        for _ in 0..8 {
            assert_eq!(original.next_u64(), resumed.next_u64());
        }

        let mut batch_19 = DeterministicRng::from_spec(spec);
        assert_ne!(batch_19.next_u64(), next_batch.next_u64());
        assert_eq!(original.draw_count(), 8);
        assert_eq!(resumed.draw_count(), 8);
    }

    #[test]
    fn rng_draw_count_tracks_public_stochastic_draws() {
        let mut rng = DeterministicRng::selection(123, 4);

        assert_eq!(rng.draw_count(), 0);
        let _ = rng.next_unit_f64();
        assert_eq!(rng.draw_count(), 1);
        let _ = rng.next_u64();
        assert_eq!(rng.draw_count(), 2);
        let _ = rng.next_u32();
        assert_eq!(rng.draw_count(), 3);
    }

    #[test]
    fn rng_fill_bytes_counts_underlying_word_draws() {
        let mut rng = DeterministicRng::misc(123, 4);
        let mut bytes = [0u8; 16];

        rng.fill_bytes(&mut bytes);

        assert_ne!(bytes, [0u8; 16]);
        assert_eq!(rng.draw_count(), 2);
    }

    const BOOT_SEED: [u8; STREAM_SEED_LEN] = [
        16, 101, 214, 49, 28, 131, 117, 31, 96, 50, 46, 62, 76, 138, 163, 176, 249, 133, 249, 3,
        120, 175, 20, 14, 175, 183, 246, 201, 113, 186, 216, 222,
    ];
    const ENTROPY_BATCH_3_JOB_0: [u8; STREAM_SEED_LEN] = [
        42, 120, 70, 143, 7, 200, 191, 91, 47, 254, 5, 203, 250, 139, 224, 121, 240, 134, 147, 105,
        144, 89, 229, 197, 71, 43, 55, 149, 160, 93, 21, 218,
    ];
    const ENTROPY_BATCH_3_JOB_5: [u8; STREAM_SEED_LEN] = [
        182, 89, 135, 122, 253, 134, 116, 55, 211, 168, 13, 34, 114, 37, 236, 191, 73, 91, 37, 92,
        237, 36, 167, 218, 204, 82, 27, 37, 68, 187, 251, 183,
    ];

    #[test]
    fn boot_and_job_entropy_seed_golden_vectors() {
        assert_eq!(derive_boot_entropy_seed(GOLDEN_SEED), BOOT_SEED);
        assert_eq!(
            derive_boot_entropy_seed(GOLDEN_SEED),
            derive_stream_seed(GOLDEN_SEED, RngPurpose::Boot, 0)
        );
        assert_eq!(
            derive_job_entropy_seed(GOLDEN_SEED, 3, 0),
            ENTROPY_BATCH_3_JOB_0
        );
        assert_eq!(
            derive_job_entropy_seed(GOLDEN_SEED, 3, 5),
            ENTROPY_BATCH_3_JOB_5
        );
        assert_ne!(
            derive_job_entropy_seed(GOLDEN_SEED, 4, 0),
            ENTROPY_BATCH_3_JOB_0
        );
        assert_ne!(
            derive_job_entropy_seed(GOLDEN_SEED + 1, 3, 0),
            ENTROPY_BATCH_3_JOB_0
        );
    }
}
