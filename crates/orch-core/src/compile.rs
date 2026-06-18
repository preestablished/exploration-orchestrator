//! Deterministic feature-map compilation and L4 coarsening.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureMap {
    pub schema_version: u32,
    pub kind: FeatureMapKind,
    pub meta: FeatureMapMeta,
    pub regions: Vec<FeatureRegion>,
    pub features: Vec<Feature>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeatureMapKind {
    FeatureMap,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureMapMeta {
    pub name: String,
    pub workload: String,
    pub game_revision: String,
    pub version: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureRegion {
    pub name: String,
    pub size: u64,
    pub layout_version: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feature {
    pub name: String,
    pub region: String,
    pub offset: u64,
    pub value_type: FeatureValueType,
    pub semantics: FeatureSemantics,
    pub stability: FeatureStability,
    pub discretize: Discretize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeatureValueType {
    U8,
    U16Le,
    U16Be,
    U32Le,
    U32Be,
    I8,
    I16Le,
    I16Be,
    I32Le,
    I32Be,
    Bitflags8,
    Bitflags16Le,
    Bitflags32Le,
    Bcd8,
    Bcd16Le,
    Bytes { width: u32 },
}

impl FeatureValueType {
    #[must_use]
    pub const fn width(&self) -> u32 {
        match self {
            Self::U8 | Self::I8 | Self::Bitflags8 | Self::Bcd8 => 1,
            Self::U16Le
            | Self::U16Be
            | Self::I16Le
            | Self::I16Be
            | Self::Bitflags16Le
            | Self::Bcd16Le => 2,
            Self::U32Le | Self::U32Be | Self::I32Le | Self::I32Be | Self::Bitflags32Le => 4,
            Self::Bytes { width } => *width,
        }
    }

    #[must_use]
    pub const fn is_integer_scalar(&self) -> bool {
        matches!(
            self,
            Self::U8
                | Self::U16Le
                | Self::U16Be
                | Self::U32Le
                | Self::U32Be
                | Self::I8
                | Self::I16Le
                | Self::I16Be
                | Self::I32Le
                | Self::I32Be
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeatureSemantics {
    Counter,
    PositionX,
    PositionY,
    RoomId,
    Health,
    Resource,
    Flags,
    Mode,
    ProgressFlag,
    Timer,
    Opaque,
    Other(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeatureStability {
    Stable,
    Volatile,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Discretize {
    None,
    Identity,
    Bucket {
        size: u64,
    },
    Threshold {
        edges: Vec<i64>,
    },
    Bits,
    Grid {
        x: String,
        y: String,
        room: String,
        cell_w: u64,
        cell_h: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledFeatureMap {
    pub meta: FeatureMapMeta,
    pub layout: CompiledLayout,
    pub fields: Vec<CompiledField>,
    pub total_len: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledLayout {
    pub ranges: Vec<ExtractRange>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractRange {
    pub region: String,
    pub layout_version: u32,
    pub offset: u64,
    pub len: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledField {
    pub name: String,
    pub region: String,
    pub source_offset: u64,
    pub width: u32,
    pub packed_offset: u32,
    pub value_type: FeatureValueType,
    pub semantics: FeatureSemantics,
    pub stability: FeatureStability,
    pub discretize: Discretize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompileError {
    WrongSchemaVersion(u32),
    DuplicateRegion(String),
    DuplicateFeature(String),
    UnknownRegion { feature: String, region: String },
    EmptyBytesFeature { feature: String },
    FeatureOutOfBounds { feature: String, region: String },
    TotalLengthOverflow,
    FeatureNotCovered { feature: String },
    InvalidGridReference { feature: String, target: String },
    InvalidGridDoubleCount { feature: String, target: String },
    InvalidGridCellSize { feature: String },
    InvalidBucketSize { feature: String },
    InvalidBitflagsDiscretize { feature: String },
    InvalidRadiusFactor,
    VersionOverflow,
    LayoutChangedByCoarsening,
}

impl fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongSchemaVersion(version) => {
                write!(
                    formatter,
                    "unsupported feature-map schema version {version}"
                )
            }
            Self::DuplicateRegion(name) => write!(formatter, "duplicate region {name}"),
            Self::DuplicateFeature(name) => write!(formatter, "duplicate feature {name}"),
            Self::UnknownRegion { feature, region } => {
                write!(
                    formatter,
                    "feature {feature} references unknown region {region}"
                )
            }
            Self::EmptyBytesFeature { feature } => {
                write!(formatter, "bytes feature {feature} must have nonzero width")
            }
            Self::FeatureOutOfBounds { feature, region } => {
                write!(
                    formatter,
                    "feature {feature} is out of bounds for region {region}"
                )
            }
            Self::TotalLengthOverflow => {
                formatter.write_str("compiled layout length overflows u32")
            }
            Self::FeatureNotCovered { feature } => {
                write!(
                    formatter,
                    "feature {feature} is not covered by compiled layout"
                )
            }
            Self::InvalidGridReference { feature, target } => {
                write!(
                    formatter,
                    "grid feature {feature} has invalid reference {target}"
                )
            }
            Self::InvalidGridDoubleCount { feature, target } => {
                write!(
                    formatter,
                    "grid feature {feature} would double-count target {target}"
                )
            }
            Self::InvalidGridCellSize { feature } => {
                write!(
                    formatter,
                    "grid feature {feature} must have nonzero cell size"
                )
            }
            Self::InvalidBucketSize { feature } => {
                write!(formatter, "bucket feature {feature} must have nonzero size")
            }
            Self::InvalidBitflagsDiscretize { feature } => {
                write!(
                    formatter,
                    "bitflags feature {feature} must use bits or none discretization"
                )
            }
            Self::InvalidRadiusFactor => {
                formatter.write_str("radius factor must be finite and positive")
            }
            Self::VersionOverflow => formatter.write_str("feature-map version overflow"),
            Self::LayoutChangedByCoarsening => {
                formatter.write_str("L4 coarsening changed the compiled layout")
            }
        }
    }
}

impl Error for CompileError {}

pub type CompileResult<T> = Result<T, CompileError>;

#[must_use]
pub fn total_feature_bytes_len(layout: &CompiledLayout) -> u32 {
    layout.ranges.iter().map(|range| range.len).sum()
}

pub fn compile_feature_map(map: &FeatureMap) -> CompileResult<CompiledFeatureMap> {
    validate_feature_map(map)?;

    let mut ranges = Vec::new();
    let mut packed_base = 0u32;

    for region in &map.regions {
        let mut intervals = map
            .features
            .iter()
            .filter(|feature| feature.region == region.name)
            .map(|feature| {
                let width = feature.value_type.width();
                (feature.offset, feature.offset + u64::from(width))
            })
            .collect::<Vec<_>>();
        intervals.sort_unstable_by_key(|(start, _)| *start);

        let mut merged: Vec<(u64, u64)> = Vec::new();
        for (start, end) in intervals {
            if let Some((_, current_end)) = merged.last_mut() {
                if start <= *current_end {
                    *current_end = (*current_end).max(end);
                    continue;
                }
            }
            merged.push((start, end));
        }

        for (start, end) in merged {
            let len = u32::try_from(end - start).map_err(|_| CompileError::TotalLengthOverflow)?;
            ranges.push(ExtractRange {
                region: region.name.clone(),
                layout_version: region.layout_version,
                offset: start,
                len,
            });
            packed_base = packed_base
                .checked_add(len)
                .ok_or(CompileError::TotalLengthOverflow)?;
        }
    }

    let layout = CompiledLayout { ranges };
    let fields = map
        .features
        .iter()
        .map(|feature| compile_field(feature, &layout))
        .collect::<CompileResult<Vec<_>>>()?;
    let total_len = total_feature_bytes_len(&layout);

    Ok(CompiledFeatureMap {
        meta: map.meta.clone(),
        layout,
        fields,
        total_len,
    })
}

pub fn coarsen_l4_preserving_layout(
    map: &FeatureMap,
    radius_factor: f64,
) -> CompileResult<FeatureMap> {
    let before = compile_feature_map(map)?.layout;
    let coarsened = coarsen_l4(map, radius_factor)?;
    let after = compile_feature_map(&coarsened)?.layout;

    if before != after {
        return Err(CompileError::LayoutChangedByCoarsening);
    }

    Ok(coarsened)
}

pub fn coarsen_l4(map: &FeatureMap, radius_factor: f64) -> CompileResult<FeatureMap> {
    if !radius_factor.is_finite() || radius_factor <= 0.0 {
        return Err(CompileError::InvalidRadiusFactor);
    }

    let mut coarsened = map.clone();
    coarsened.meta.version = coarsened
        .meta
        .version
        .checked_add(1)
        .ok_or(CompileError::VersionOverflow)?;

    for feature in &mut coarsened.features {
        match &mut feature.discretize {
            Discretize::Bucket { size } => {
                *size = scale_positive(*size, radius_factor)?;
            }
            Discretize::Grid { cell_w, cell_h, .. } => {
                *cell_w = scale_positive(*cell_w, radius_factor)?;
                *cell_h = scale_positive(*cell_h, radius_factor)?;
            }
            Discretize::None
            | Discretize::Identity
            | Discretize::Threshold { .. }
            | Discretize::Bits => {}
        }
    }

    Ok(coarsened)
}

fn validate_feature_map(map: &FeatureMap) -> CompileResult<()> {
    if map.schema_version != 1 {
        return Err(CompileError::WrongSchemaVersion(map.schema_version));
    }

    let mut region_names = HashSet::new();
    for region in &map.regions {
        if !region_names.insert(region.name.as_str()) {
            return Err(CompileError::DuplicateRegion(region.name.clone()));
        }
    }

    let mut feature_names = HashSet::new();
    for feature in &map.features {
        if !feature_names.insert(feature.name.as_str()) {
            return Err(CompileError::DuplicateFeature(feature.name.clone()));
        }
        validate_feature_bounds(map, feature)?;
        validate_discretize(map, feature)?;
    }

    Ok(())
}

fn validate_feature_bounds(map: &FeatureMap, feature: &Feature) -> CompileResult<()> {
    let region = map
        .regions
        .iter()
        .find(|region| region.name == feature.region)
        .ok_or_else(|| CompileError::UnknownRegion {
            feature: feature.name.clone(),
            region: feature.region.clone(),
        })?;

    let width = feature.value_type.width();
    if width == 0 {
        return Err(CompileError::EmptyBytesFeature {
            feature: feature.name.clone(),
        });
    }

    let end = feature
        .offset
        .checked_add(u64::from(width))
        .ok_or_else(|| CompileError::FeatureOutOfBounds {
            feature: feature.name.clone(),
            region: feature.region.clone(),
        })?;
    if end > region.size {
        return Err(CompileError::FeatureOutOfBounds {
            feature: feature.name.clone(),
            region: feature.region.clone(),
        });
    }

    Ok(())
}

fn validate_discretize(map: &FeatureMap, feature: &Feature) -> CompileResult<()> {
    if matches!(
        feature.value_type,
        FeatureValueType::Bitflags8
            | FeatureValueType::Bitflags16Le
            | FeatureValueType::Bitflags32Le
    ) && !matches!(feature.discretize, Discretize::Bits | Discretize::None)
    {
        return Err(CompileError::InvalidBitflagsDiscretize {
            feature: feature.name.clone(),
        });
    }

    match &feature.discretize {
        Discretize::Bucket { size } if *size == 0 => Err(CompileError::InvalidBucketSize {
            feature: feature.name.clone(),
        }),
        Discretize::Grid {
            x,
            y,
            room,
            cell_w,
            cell_h,
        } => {
            if *cell_w == 0 || *cell_h == 0 {
                return Err(CompileError::InvalidGridCellSize {
                    feature: feature.name.clone(),
                });
            }
            validate_grid_target(map, feature, x, true)?;
            validate_grid_target(map, feature, y, true)?;
            validate_grid_target(map, feature, room, false)
        }
        Discretize::None
        | Discretize::Identity
        | Discretize::Bucket { .. }
        | Discretize::Threshold { .. }
        | Discretize::Bits => Ok(()),
    }
}

fn validate_grid_target(
    map: &FeatureMap,
    feature: &Feature,
    target: &str,
    must_be_integer: bool,
) -> CompileResult<()> {
    let target_feature = map
        .features
        .iter()
        .find(|candidate| candidate.name == target)
        .ok_or_else(|| CompileError::InvalidGridReference {
            feature: feature.name.clone(),
            target: target.to_owned(),
        })?;

    if must_be_integer && !target_feature.value_type.is_integer_scalar() {
        return Err(CompileError::InvalidGridReference {
            feature: feature.name.clone(),
            target: target.to_owned(),
        });
    }
    if must_be_integer
        && target_feature.name != feature.name
        && target_feature.discretize != Discretize::None
    {
        return Err(CompileError::InvalidGridDoubleCount {
            feature: feature.name.clone(),
            target: target.to_owned(),
        });
    }
    if !must_be_integer && target_feature.stability != FeatureStability::Stable {
        return Err(CompileError::InvalidGridReference {
            feature: feature.name.clone(),
            target: target.to_owned(),
        });
    }

    Ok(())
}

fn compile_field(feature: &Feature, layout: &CompiledLayout) -> CompileResult<CompiledField> {
    let width = feature.value_type.width();
    let range = layout
        .ranges
        .iter()
        .scan(0u32, |packed_base, range| {
            let current_base = *packed_base;
            *packed_base = packed_base.checked_add(range.len)?;
            Some((current_base, range))
        })
        .find(|(_, range)| {
            range.region == feature.region
                && feature.offset >= range.offset
                && feature.offset + u64::from(width) <= range.offset + u64::from(range.len)
        })
        .ok_or_else(|| CompileError::FeatureNotCovered {
            feature: feature.name.clone(),
        })?;

    let packed_offset = range
        .0
        .checked_add(u32::try_from(feature.offset - range.1.offset).map_err(|_| {
            CompileError::FeatureNotCovered {
                feature: feature.name.clone(),
            }
        })?)
        .ok_or(CompileError::TotalLengthOverflow)?;

    Ok(CompiledField {
        name: feature.name.clone(),
        region: feature.region.clone(),
        source_offset: feature.offset,
        width,
        packed_offset,
        value_type: feature.value_type.clone(),
        semantics: feature.semantics.clone(),
        stability: feature.stability,
        discretize: feature.discretize.clone(),
    })
}

fn scale_positive(value: u64, radius_factor: f64) -> CompileResult<u64> {
    let scaled = (value as f64) * radius_factor;
    if !scaled.is_finite() || scaled > u64::MAX as f64 {
        return Err(CompileError::InvalidRadiusFactor);
    }
    Ok((scaled.ceil() as u64).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_feature_map_emits_region_ordered_minimal_ranges() {
        let compiled = compile_feature_map(&sample_map()).expect("compile sample map");

        let expected_ranges = vec![
            ExtractRange {
                region: "wram".to_owned(),
                layout_version: 7,
                offset: 0x10,
                len: 2,
            },
            ExtractRange {
                region: "wram".to_owned(),
                layout_version: 7,
                offset: 0x20,
                len: 4,
            },
            ExtractRange {
                region: "framebuffer".to_owned(),
                layout_version: 3,
                offset: 4,
                len: 3,
            },
        ];

        assert_eq!(compiled.layout.ranges, expected_ranges);
        assert_eq!(compiled.total_len, 9);
        assert_eq!(compiled.fields[0].packed_offset, 0);
        assert_eq!(compiled.fields[1].packed_offset, 2);
        assert_eq!(compiled.fields[2].packed_offset, 4);
        assert_eq!(compiled.fields[3].packed_offset, 6);
    }

    #[test]
    fn compile_feature_map_has_stable_golden_layout_bytes() {
        let map = sample_map();
        let compiled_a = compile_feature_map(&map).expect("compile first");
        let compiled_b = compile_feature_map(&map).expect("compile second");
        let expected_ranges = vec![
            ExtractRange {
                region: "wram".to_owned(),
                layout_version: 7,
                offset: 0x10,
                len: 2,
            },
            ExtractRange {
                region: "wram".to_owned(),
                layout_version: 7,
                offset: 0x20,
                len: 4,
            },
            ExtractRange {
                region: "framebuffer".to_owned(),
                layout_version: 3,
                offset: 4,
                len: 3,
            },
        ];

        assert_eq!(
            postcard::to_allocvec(&compiled_a.layout.ranges).expect("layout bytes"),
            postcard::to_allocvec(&expected_ranges).expect("expected layout bytes")
        );
        assert_eq!(
            postcard::to_allocvec(&compiled_a.layout.ranges).expect("layout bytes"),
            postcard::to_allocvec(&compiled_b.layout.ranges).expect("layout bytes")
        );
        assert_eq!(compiled_a.total_len, compiled_b.total_len);
    }

    #[test]
    fn l4_coarsening_changes_only_discretize_and_version() {
        let map = sample_map();
        let before = compile_feature_map(&map).expect("compile original");
        let coarsened = coarsen_l4_preserving_layout(&map, 2.0).expect("coarsen");
        let after = compile_feature_map(&coarsened).expect("compile coarsened");

        assert_eq!(coarsened.meta.version, map.meta.version + 1);
        assert_eq!(before.layout, after.layout);
        assert_eq!(before.total_len, after.total_len);
        assert_eq!(coarsened.features[0].discretize, map.features[0].discretize);
        assert_eq!(
            coarsened.features[1].discretize,
            Discretize::Grid {
                x: "player_x".to_owned(),
                y: "player_y".to_owned(),
                room: "room_id".to_owned(),
                cell_w: 32,
                cell_h: 32,
            }
        );
        assert_eq!(
            coarsened.features[3].discretize,
            Discretize::Bucket { size: 8 }
        );
        for (original, changed) in map.features.iter().zip(coarsened.features.iter()) {
            assert_eq!(original.name, changed.name);
            assert_eq!(original.region, changed.region);
            assert_eq!(original.offset, changed.offset);
            assert_eq!(original.value_type, changed.value_type);
        }
    }

    #[test]
    fn compile_feature_map_rejects_invalid_layout_inputs() {
        let mut map = sample_map();
        map.features[0].offset = 4096;

        let error = compile_feature_map(&map).expect_err("out of bounds");
        assert!(matches!(error, CompileError::FeatureOutOfBounds { .. }));

        let mut map = sample_map();
        map.features.push(map.features[0].clone());
        let error = compile_feature_map(&map).expect_err("duplicate feature");
        assert!(matches!(error, CompileError::DuplicateFeature(_)));

        let mut map = sample_map();
        map.features[2].discretize = Discretize::Identity;
        let error = compile_feature_map(&map).expect_err("grid y double count");
        assert!(matches!(error, CompileError::InvalidGridDoubleCount { .. }));
    }

    fn sample_map() -> FeatureMap {
        FeatureMap {
            schema_version: 1,
            kind: FeatureMapKind::FeatureMap,
            meta: FeatureMapMeta {
                name: "demo-game".to_owned(),
                workload: "refwork-demo".to_owned(),
                game_revision: "operator-set".to_owned(),
                version: 3,
            },
            regions: vec![
                FeatureRegion {
                    name: "wram".to_owned(),
                    size: 128,
                    layout_version: 7,
                },
                FeatureRegion {
                    name: "framebuffer".to_owned(),
                    size: 64,
                    layout_version: 3,
                },
            ],
            features: vec![
                Feature {
                    name: "room_id".to_owned(),
                    region: "wram".to_owned(),
                    offset: 0x10,
                    value_type: FeatureValueType::U16Le,
                    semantics: FeatureSemantics::RoomId,
                    stability: FeatureStability::Stable,
                    discretize: Discretize::Identity,
                },
                Feature {
                    name: "player_x".to_owned(),
                    region: "wram".to_owned(),
                    offset: 0x20,
                    value_type: FeatureValueType::U16Le,
                    semantics: FeatureSemantics::PositionX,
                    stability: FeatureStability::Volatile,
                    discretize: Discretize::Grid {
                        x: "player_x".to_owned(),
                        y: "player_y".to_owned(),
                        room: "room_id".to_owned(),
                        cell_w: 16,
                        cell_h: 16,
                    },
                },
                Feature {
                    name: "player_y".to_owned(),
                    region: "wram".to_owned(),
                    offset: 0x22,
                    value_type: FeatureValueType::U16Le,
                    semantics: FeatureSemantics::PositionY,
                    stability: FeatureStability::Volatile,
                    discretize: Discretize::None,
                },
                Feature {
                    name: "health".to_owned(),
                    region: "framebuffer".to_owned(),
                    offset: 4,
                    value_type: FeatureValueType::Bytes { width: 3 },
                    semantics: FeatureSemantics::Health,
                    stability: FeatureStability::Stable,
                    discretize: Discretize::Bucket { size: 4 },
                },
            ],
        }
    }
}
