//! Deterministic feature-map compilation and L4 coarsening.

use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fmt;
use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

pub type RegionLayouts = BTreeMap<String, RegionLayout>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureMap {
    pub schema_version: u32,
    pub kind: FeatureMapKind,
    pub meta: FeatureMapMeta,
    pub regions: Vec<FeatureRegion>,
    pub features: Vec<Feature>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, ExtraValue>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FeatureMapKind {
    FeatureMap,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureMapMeta {
    pub name: String,
    pub workload: String,
    pub game_revision: String,
    pub version: u32,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, ExtraValue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureRegion {
    pub name: String,
    pub size: u64,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, ExtraValue>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionLayout {
    pub size: u64,
    pub layout_version: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feature {
    pub name: String,
    pub region: String,
    pub offset: u64,
    #[serde(rename = "type")]
    pub value_type: FeatureValueType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    pub semantics: FeatureSemantics,
    pub stability: FeatureStability,
    #[serde(default)]
    pub discretize: Discretize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_when: Option<FeaturePredicate>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, ExtraValue>,
}

impl Feature {
    pub fn width(&self) -> CompileResult<u32> {
        match self.value_type.derived_width() {
            Some(derived) => {
                if let Some(width) = self.width {
                    if width != derived {
                        return Err(CompileError::WidthMismatch {
                            feature: self.name.clone(),
                        });
                    }
                }
                Ok(derived)
            }
            None => {
                let width = self.width.ok_or_else(|| CompileError::MissingBytesWidth {
                    feature: self.name.clone(),
                })?;
                if width == 0 {
                    return Err(CompileError::EmptyBytesFeature {
                        feature: self.name.clone(),
                    });
                }
                Ok(width)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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
    Bytes,
}

impl FeatureValueType {
    #[must_use]
    pub const fn derived_width(self) -> Option<u32> {
        match self {
            Self::U8 | Self::I8 | Self::Bitflags8 | Self::Bcd8 => Some(1),
            Self::U16Le
            | Self::U16Be
            | Self::I16Le
            | Self::I16Be
            | Self::Bitflags16Le
            | Self::Bcd16Le => Some(2),
            Self::U32Le | Self::U32Be | Self::I32Le | Self::I32Be | Self::Bitflags32Le => Some(4),
            Self::Bytes => None,
        }
    }

    #[must_use]
    pub const fn is_integer_scalar(self) -> bool {
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

    #[must_use]
    pub const fn is_bitflags(self) -> bool {
        matches!(
            self,
            Self::Bitflags8 | Self::Bitflags16Le | Self::Bitflags32Le
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FeatureSemantics(pub String);

impl FeatureSemantics {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeatureStability {
    Stable,
    Volatile,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Discretize {
    #[default]
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
pub struct FeaturePredicate {
    pub feature: String,
    pub op: PredicateOp,
    pub value: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PredicateOp {
    Eq,
    Ne,
    Ge,
    Le,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExtraValue {
    Bool(bool),
    I64(i64),
    U64(u64),
    String(String),
    Seq(Vec<ExtraValue>),
    Map(BTreeMap<String, ExtraValue>),
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
    pub field_index: u32,
    pub name: String,
    pub region: String,
    pub source_offset: u64,
    pub width: u32,
    pub packed_offset: u32,
    pub value_type: FeatureValueType,
    pub semantics: FeatureSemantics,
    pub stability: FeatureStability,
    pub discretize: Discretize,
    pub valid_when: Option<FeaturePredicate>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadiusFactor {
    pub numerator: u64,
    pub denominator: NonZeroU64,
}

impl RadiusFactor {
    pub fn new(numerator: u64, denominator: u64) -> CompileResult<Self> {
        let denominator = NonZeroU64::new(denominator).ok_or(CompileError::InvalidRadiusFactor)?;
        if numerator < denominator.get() {
            return Err(CompileError::InvalidRadiusFactor);
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    pub fn integer(value: u64) -> CompileResult<Self> {
        Self::new(value, 1)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompileError {
    WrongSchemaVersion(u32),
    InvalidName { field: &'static str, name: String },
    DuplicateRegion(String),
    DuplicateFeature(String),
    UnknownRegion { feature: String, region: String },
    UnknownRegionLayout(String),
    RegionSizeMismatch { region: String },
    MissingBytesWidth { feature: String },
    EmptyBytesFeature { feature: String },
    WidthMismatch { feature: String },
    FeatureOutOfBounds { feature: String, region: String },
    TotalLengthOverflow,
    FeatureNotCovered { feature: String },
    InvalidGridReference { feature: String, target: String },
    InvalidGridDoubleCount { feature: String, target: String },
    InvalidGridCellSize { feature: String },
    InvalidBucketSize { feature: String },
    InvalidBitflagsDiscretize { feature: String },
    InvalidRadiusFactor,
    RadiusOverflow,
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
            Self::InvalidName { field, name } => write!(formatter, "invalid {field} name {name}"),
            Self::DuplicateRegion(name) => write!(formatter, "duplicate region {name}"),
            Self::DuplicateFeature(name) => write!(formatter, "duplicate feature {name}"),
            Self::UnknownRegion { feature, region } => {
                write!(
                    formatter,
                    "feature {feature} references unknown region {region}"
                )
            }
            Self::UnknownRegionLayout(region) => {
                write!(formatter, "missing layout metadata for region {region}")
            }
            Self::RegionSizeMismatch { region } => {
                write!(
                    formatter,
                    "feature-map region {region} exceeds manifest region size"
                )
            }
            Self::MissingBytesWidth { feature } => {
                write!(formatter, "bytes feature {feature} must declare width")
            }
            Self::EmptyBytesFeature { feature } => {
                write!(formatter, "bytes feature {feature} must have nonzero width")
            }
            Self::WidthMismatch { feature } => {
                write!(formatter, "feature {feature} declares a mismatched width")
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
            Self::InvalidRadiusFactor => formatter.write_str("radius factor must be at least 1/1"),
            Self::RadiusOverflow => formatter.write_str("radius factor scaling overflows u64"),
            Self::VersionOverflow => formatter.write_str("feature-map version overflow"),
            Self::LayoutChangedByCoarsening => {
                formatter.write_str("L4 coarsening changed the compiled layout")
            }
        }
    }
}

impl Error for CompileError {}

pub type CompileResult<T> = Result<T, CompileError>;

pub fn total_feature_bytes_len(layout: &CompiledLayout) -> CompileResult<u32> {
    layout.ranges.iter().try_fold(0u32, |total, range| {
        total
            .checked_add(range.len)
            .ok_or(CompileError::TotalLengthOverflow)
    })
}

pub fn compile_feature_map(
    map: &FeatureMap,
    region_layouts: &RegionLayouts,
) -> CompileResult<CompiledFeatureMap> {
    validate_feature_map(map, region_layouts)?;

    let mut ranges = Vec::new();
    for region in &map.regions {
        let region_layout = region_layouts
            .get(&region.name)
            .ok_or_else(|| CompileError::UnknownRegionLayout(region.name.clone()))?;
        let mut intervals = Vec::new();
        for feature in map
            .features
            .iter()
            .filter(|feature| feature.region == region.name)
        {
            let width = feature.width()?;
            intervals.push((
                feature.offset,
                feature
                    .offset
                    .checked_add(u64::from(width))
                    .ok_or_else(|| CompileError::FeatureOutOfBounds {
                        feature: feature.name.clone(),
                        region: feature.region.clone(),
                    })?,
            ));
        }
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
                layout_version: region_layout.layout_version,
                offset: start,
                len,
            });
        }
    }

    let layout = CompiledLayout { ranges };
    let total_len = total_feature_bytes_len(&layout)?;
    let fields = map
        .features
        .iter()
        .enumerate()
        .map(|(index, feature)| compile_field(index, feature, &layout))
        .collect::<CompileResult<Vec<_>>>()?;

    Ok(CompiledFeatureMap {
        meta: map.meta.clone(),
        layout,
        fields,
        total_len,
    })
}

pub fn coarsen_l4_preserving_layout(
    map: &FeatureMap,
    region_layouts: &RegionLayouts,
    radius_factor: RadiusFactor,
) -> CompileResult<FeatureMap> {
    let before = compile_feature_map(map, region_layouts)?.layout;
    let coarsened = coarsen_l4(map, region_layouts, radius_factor)?;
    let after = compile_feature_map(&coarsened, region_layouts)?.layout;

    if before != after {
        return Err(CompileError::LayoutChangedByCoarsening);
    }

    Ok(coarsened)
}

pub fn coarsen_l4(
    map: &FeatureMap,
    region_layouts: &RegionLayouts,
    radius_factor: RadiusFactor,
) -> CompileResult<FeatureMap> {
    validate_feature_map(map, region_layouts)?;

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

fn validate_feature_map(map: &FeatureMap, region_layouts: &RegionLayouts) -> CompileResult<()> {
    if map.schema_version != 1 {
        return Err(CompileError::WrongSchemaVersion(map.schema_version));
    }
    validate_name("meta.name", &map.meta.name, true)?;

    let mut region_names = HashSet::new();
    for region in &map.regions {
        validate_name("region", &region.name, false)?;
        if !region_names.insert(region.name.as_str()) {
            return Err(CompileError::DuplicateRegion(region.name.clone()));
        }
        let region_layout = region_layouts
            .get(&region.name)
            .ok_or_else(|| CompileError::UnknownRegionLayout(region.name.clone()))?;
        if region.size > region_layout.size {
            return Err(CompileError::RegionSizeMismatch {
                region: region.name.clone(),
            });
        }
    }

    let mut feature_names = HashSet::new();
    for feature in &map.features {
        validate_name("feature", &feature.name, false)?;
        if !feature_names.insert(feature.name.as_str()) {
            return Err(CompileError::DuplicateFeature(feature.name.clone()));
        }
        validate_feature_bounds(map, feature)?;
        validate_discretize(map, feature)?;
    }

    Ok(())
}

fn validate_name(field: &'static str, name: &str, allow_hyphen: bool) -> CompileResult<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(CompileError::InvalidName {
            field,
            name: name.to_owned(),
        });
    };
    if !first.is_ascii_lowercase() {
        return Err(CompileError::InvalidName {
            field,
            name: name.to_owned(),
        });
    }
    if !chars.all(|ch| {
        ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || (allow_hyphen && ch == '-')
    }) {
        return Err(CompileError::InvalidName {
            field,
            name: name.to_owned(),
        });
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

    let width = feature.width()?;
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
    if feature.value_type.is_bitflags()
        && !matches!(feature.discretize, Discretize::Bits | Discretize::None)
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

fn compile_field(
    index: usize,
    feature: &Feature,
    layout: &CompiledLayout,
) -> CompileResult<CompiledField> {
    let width = feature.width()?;
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
        field_index: u32::try_from(index).map_err(|_| CompileError::TotalLengthOverflow)?,
        name: feature.name.clone(),
        region: feature.region.clone(),
        source_offset: feature.offset,
        width,
        packed_offset,
        value_type: feature.value_type,
        semantics: feature.semantics.clone(),
        stability: feature.stability,
        discretize: feature.discretize.clone(),
        valid_when: feature.valid_when.clone(),
    })
}

fn scale_positive(value: u64, radius_factor: RadiusFactor) -> CompileResult<u64> {
    let product = u128::from(value) * u128::from(radius_factor.numerator);
    let denominator = u128::from(radius_factor.denominator.get());
    let scaled = product.div_ceil(denominator);
    if scaled > u128::from(u64::MAX) {
        return Err(CompileError::RadiusOverflow);
    }
    Ok((scaled as u64).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_feature_map_emits_region_ordered_minimal_ranges() {
        let compiled = compile_feature_map(&sample_map(), &sample_region_layouts())
            .expect("compile sample map");

        let expected_ranges = expected_ranges();

        assert_eq!(compiled.layout.ranges, expected_ranges);
        assert_eq!(compiled.total_len, 9);
        assert_eq!(compiled.fields[0].field_index, 0);
        assert_eq!(compiled.fields[0].packed_offset, 0);
        assert_eq!(compiled.fields[1].packed_offset, 2);
        assert_eq!(compiled.fields[2].packed_offset, 4);
        assert_eq!(compiled.fields[3].packed_offset, 6);
    }

    #[test]
    fn identical_feature_map_bytes_compile_to_identical_layout_and_total_len() {
        let bytes = br#"{
            "schema_version": 1,
            "kind": "feature-map",
            "meta": {
                "name": "demo-game",
                "workload": "refwork-demo",
                "game_revision": "operator-set",
                "version": 3,
                "authors": ["operator"]
            },
            "regions": [
                {"name": "wram", "size": 128},
                {"name": "framebuffer", "size": 64}
            ],
            "features": [
                {
                    "name": "room_id",
                    "region": "wram",
                    "offset": 16,
                    "type": "u16le",
                    "semantics": "room_id",
                    "stability": "stable",
                    "discretize": {"kind": "identity"},
                    "description": "Current room"
                },
                {
                    "name": "player_x",
                    "region": "wram",
                    "offset": 32,
                    "type": "u16le",
                    "semantics": "position_x",
                    "stability": "volatile",
                    "discretize": {
                        "kind": "grid",
                        "x": "player_x",
                        "y": "player_y",
                        "room": "room_id",
                        "cell_w": 16,
                        "cell_h": 16
                    }
                },
                {
                    "name": "player_y",
                    "region": "wram",
                    "offset": 34,
                    "type": "u16le",
                    "semantics": "position_y",
                    "stability": "volatile"
                },
                {
                    "name": "health",
                    "region": "framebuffer",
                    "offset": 4,
                    "type": "bytes",
                    "width": 3,
                    "semantics": "health",
                    "stability": "stable",
                    "discretize": {"kind": "bucket", "size": 4},
                    "valid_when": {"feature": "room_id", "op": "ge", "value": 0}
                }
            ]
        }"#;
        let map_a: FeatureMap = serde_json::from_slice(bytes).expect("parse first");
        let map_b: FeatureMap = serde_json::from_slice(bytes).expect("parse second");
        let layouts = sample_region_layouts();

        let compiled_a = compile_feature_map(&map_a, &layouts).expect("compile first");
        let compiled_b = compile_feature_map(&map_b, &layouts).expect("compile second");

        let expected_ranges = expected_ranges();
        assert_eq!(
            postcard::to_allocvec(&(&compiled_a.layout.ranges, compiled_a.total_len))
                .expect("layout bytes"),
            postcard::to_allocvec(&(&expected_ranges, 9u32)).expect("expected layout bytes")
        );
        assert_eq!(
            postcard::to_allocvec(&(&compiled_a.layout.ranges, compiled_a.total_len))
                .expect("layout bytes"),
            postcard::to_allocvec(&(&compiled_b.layout.ranges, compiled_b.total_len))
                .expect("layout bytes")
        );
        assert_eq!(
            total_feature_bytes_len(&compiled_b.layout).expect("checked total"),
            compiled_b.total_len
        );
    }

    #[test]
    fn l4_coarsening_changes_only_discretize_and_version() {
        let map = sample_map();
        let layouts = sample_region_layouts();
        let before = compile_feature_map(&map, &layouts).expect("compile original");
        let factor = RadiusFactor::integer(2).expect("factor");
        let coarsened = coarsen_l4_preserving_layout(&map, &layouts, factor)
            .expect("coarsen preserving layout");
        let after = compile_feature_map(&coarsened, &layouts).expect("compile coarsened");

        let mut expected = map.clone();
        expected.meta.version += 1;
        expected.features[1].discretize = Discretize::Grid {
            x: "player_x".to_owned(),
            y: "player_y".to_owned(),
            room: "room_id".to_owned(),
            cell_w: 32,
            cell_h: 32,
        };
        expected.features[3].discretize = Discretize::Bucket { size: 8 };

        assert_eq!(coarsened, expected);
        assert_eq!(before.layout, after.layout);
        assert_eq!(before.total_len, after.total_len);
    }

    #[test]
    fn compile_feature_map_rejects_invalid_layout_inputs() {
        let layouts = sample_region_layouts();
        let mut map = sample_map();
        map.features[0].offset = 4096;

        let error = compile_feature_map(&map, &layouts).expect_err("out of bounds");
        assert!(matches!(error, CompileError::FeatureOutOfBounds { .. }));

        let mut map = sample_map();
        map.features.push(map.features[0].clone());
        let error = compile_feature_map(&map, &layouts).expect_err("duplicate feature");
        assert!(matches!(error, CompileError::DuplicateFeature(_)));

        let mut map = sample_map();
        map.features[2].discretize = Discretize::Identity;
        let error = compile_feature_map(&map, &layouts).expect_err("grid y double count");
        assert!(matches!(error, CompileError::InvalidGridDoubleCount { .. }));

        let layout = CompiledLayout {
            ranges: vec![
                ExtractRange {
                    region: "a".to_owned(),
                    layout_version: 1,
                    offset: 0,
                    len: u32::MAX,
                },
                ExtractRange {
                    region: "b".to_owned(),
                    layout_version: 1,
                    offset: 0,
                    len: 1,
                },
            ],
        };
        assert!(matches!(
            total_feature_bytes_len(&layout),
            Err(CompileError::TotalLengthOverflow)
        ));
    }

    #[test]
    fn l4_radius_factor_is_one_way_and_integer_safe() {
        assert!(matches!(
            RadiusFactor::new(1, 2),
            Err(CompileError::InvalidRadiusFactor)
        ));

        let mut map = sample_map();
        map.features[3].discretize = Discretize::Bucket { size: u64::MAX };
        let error = coarsen_l4(
            &map,
            &sample_region_layouts(),
            RadiusFactor::integer(2).expect("factor"),
        )
        .expect_err("overflow");
        assert!(matches!(error, CompileError::RadiusOverflow));
    }

    fn expected_ranges() -> Vec<ExtractRange> {
        vec![
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
        ]
    }

    fn sample_region_layouts() -> RegionLayouts {
        BTreeMap::from([
            (
                "wram".to_owned(),
                RegionLayout {
                    size: 128,
                    layout_version: 7,
                },
            ),
            (
                "framebuffer".to_owned(),
                RegionLayout {
                    size: 64,
                    layout_version: 3,
                },
            ),
        ])
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
                extra: BTreeMap::from([(
                    "authors".to_owned(),
                    ExtraValue::Seq(vec![ExtraValue::String("operator".to_owned())]),
                )]),
            },
            regions: vec![
                FeatureRegion {
                    name: "wram".to_owned(),
                    size: 128,
                    extra: BTreeMap::new(),
                },
                FeatureRegion {
                    name: "framebuffer".to_owned(),
                    size: 64,
                    extra: BTreeMap::new(),
                },
            ],
            features: vec![
                Feature {
                    name: "room_id".to_owned(),
                    region: "wram".to_owned(),
                    offset: 0x10,
                    value_type: FeatureValueType::U16Le,
                    width: None,
                    semantics: FeatureSemantics::new("room_id"),
                    stability: FeatureStability::Stable,
                    discretize: Discretize::Identity,
                    valid_when: None,
                    extra: BTreeMap::from([(
                        "description".to_owned(),
                        ExtraValue::String("Current room".to_owned()),
                    )]),
                },
                Feature {
                    name: "player_x".to_owned(),
                    region: "wram".to_owned(),
                    offset: 0x20,
                    value_type: FeatureValueType::U16Le,
                    width: None,
                    semantics: FeatureSemantics::new("position_x"),
                    stability: FeatureStability::Volatile,
                    discretize: Discretize::Grid {
                        x: "player_x".to_owned(),
                        y: "player_y".to_owned(),
                        room: "room_id".to_owned(),
                        cell_w: 16,
                        cell_h: 16,
                    },
                    valid_when: None,
                    extra: BTreeMap::new(),
                },
                Feature {
                    name: "player_y".to_owned(),
                    region: "wram".to_owned(),
                    offset: 0x22,
                    value_type: FeatureValueType::U16Le,
                    width: None,
                    semantics: FeatureSemantics::new("position_y"),
                    stability: FeatureStability::Volatile,
                    discretize: Discretize::None,
                    valid_when: None,
                    extra: BTreeMap::new(),
                },
                Feature {
                    name: "health".to_owned(),
                    region: "framebuffer".to_owned(),
                    offset: 4,
                    value_type: FeatureValueType::Bytes,
                    width: Some(3),
                    semantics: FeatureSemantics::new("health"),
                    stability: FeatureStability::Stable,
                    discretize: Discretize::Bucket { size: 4 },
                    valid_when: Some(FeaturePredicate {
                        feature: "room_id".to_owned(),
                        op: PredicateOp::Ge,
                        value: 0,
                    }),
                    extra: BTreeMap::new(),
                },
            ],
            extra: BTreeMap::new(),
        }
    }
}
