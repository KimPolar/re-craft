//! Structure-set candidate chunks for 26.3 random-spread placements.
//! Candidate placement does not mean a structure start is valid or generated.
use crate::packs::{ResourceId, ResourceProvider};
use anyhow::{Context, Result, bail, ensure};
use native_core::random::LegacyRandom;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Spread {
    Linear,
    Triangular,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Reduction {
    Default,
    Legacy1,
    Legacy2,
    Legacy3,
}
#[derive(Clone, Debug)]
pub struct RandomSpreadPlacement {
    spacing: i32,
    separation: i32,
    spread: Spread,
    rules: PlacementRules,
}
#[derive(Clone, Debug)]
struct PlacementRules {
    salt: i32,
    frequency: f32,
    reduction: Reduction,
    locate_offset: [i32; 3],
    exclusion: Option<(ResourceId, i32)>,
}
fn integer(value: &Value, name: &str) -> Result<i32> {
    let v = value
        .get(name)
        .and_then(Value::as_i64)
        .with_context(|| format!("missing or invalid {name}"))?;
    Ok(i32::try_from(v)?)
}
fn salted(seed: i64, x: i32, z: i32, salt: i32) -> LegacyRandom {
    LegacyRandom::new(
        i64::from(x)
            .wrapping_mul(341_873_128_712)
            .wrapping_add(i64::from(z).wrapping_mul(132_897_987_541))
            .wrapping_add(seed)
            .wrapping_add(i64::from(salt)),
    )
}
pub(crate) fn large_feature(seed: i64, x: i32, z: i32) -> LegacyRandom {
    let mut r = LegacyRandom::new(seed);
    let a = r.next_long();
    let b = r.next_long();
    LegacyRandom::new(i64::from(x).wrapping_mul(a) ^ i64::from(z).wrapping_mul(b) ^ seed)
}
impl PlacementRules {
    fn decode(value: &Value) -> Result<Self> {
        let salt = integer(value, "salt")?;
        ensure!(salt >= 0, "invalid structure salt");
        let frequency = value
            .get("frequency")
            .map(|v| v.as_f64().context("invalid frequency"))
            .transpose()?
            .unwrap_or(1.0) as f32;
        ensure!(
            frequency.is_finite() && (0.0..=1.0).contains(&frequency),
            "invalid structure frequency"
        );
        let reduction = match value
            .get("frequency_reduction_method")
            .map(|v| v.as_str().context("invalid frequency reduction"))
            .transpose()?
            .unwrap_or("default")
        {
            "default" => Reduction::Default,
            "legacy_type_1" => Reduction::Legacy1,
            "legacy_type_2" => Reduction::Legacy2,
            "legacy_type_3" => Reduction::Legacy3,
            name => bail!("unsupported frequency reduction {name}"),
        };
        let mut locate_offset = [0; 3];
        if let Some(offset) = value.get("locate_offset") {
            let offset = offset.as_array().context("invalid locate offset")?;
            ensure!(offset.len() == 3, "invalid locate offset");
            for i in 0..3 {
                locate_offset[i] =
                    i32::try_from(offset[i].as_i64().context("invalid locate offset")?)?;
                ensure!(
                    (-16..=16).contains(&locate_offset[i]),
                    "locate offset out of range"
                );
            }
        }
        let exclusion = value
            .get("exclusion_zone")
            .map(|zone| -> Result<_> {
                let name = zone
                    .get("other_set")
                    .and_then(Value::as_str)
                    .context("inline or invalid exclusion structure set is unsupported")?;
                let range = integer(zone, "chunk_count")?;
                ensure!((1..=16).contains(&range), "invalid exclusion range");
                Ok((ResourceId::parse(name)?, range))
            })
            .transpose()?;
        Ok(Self {
            salt,
            frequency,
            reduction,
            locate_offset,
            exclusion,
        })
    }
    fn passes_frequency(&self, seed: i64, source: [i32; 2]) -> bool {
        if self.frequency >= 1.0 {
            return true;
        }
        let [x, z] = source;
        match self.reduction {
            Reduction::Default => salted(seed, self.salt, x, z).next_float() < self.frequency,
            Reduction::Legacy1 => {
                let cx = x >> 4;
                let cz = z >> 4;
                let mut r = LegacyRandom::new(i64::from(cx ^ (cz.wrapping_shl(4))) ^ seed);
                r.next_int();
                r.next_int_bound((1.0f32 / self.frequency) as i32).unwrap() == 0
            }
            Reduction::Legacy2 => salted(seed, x, z, 10_387_320).next_float() < self.frequency,
            Reduction::Legacy3 => {
                large_feature(seed, x, z).next_double() < f64::from(self.frequency)
            }
        }
    }
    fn locate_position(&self, chunk: [i32; 2]) -> [i32; 3] {
        [
            chunk[0]
                .wrapping_mul(16)
                .wrapping_add(self.locate_offset[0]),
            self.locate_offset[1],
            chunk[1]
                .wrapping_mul(16)
                .wrapping_add(self.locate_offset[2]),
        ]
    }
}

impl RandomSpreadPlacement {
    pub fn decode(value: &Value) -> Result<Self> {
        let kind = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing placement type")?,
        )?;
        ensure!(
            kind.namespace == "minecraft" && kind.path == "random_spread",
            "unsupported structure placement {}:{}",
            kind.namespace,
            kind.path
        );
        let spacing = integer(value, "spacing")?;
        let separation = integer(value, "separation")?;
        ensure!(
            (0..=4096).contains(&spacing)
                && (0..=4096).contains(&separation)
                && spacing > separation,
            "invalid structure spacing or separation"
        );
        let spread = match value
            .get("spread_type")
            .map(|v| v.as_str().context("invalid spread type"))
            .transpose()?
            .unwrap_or("linear")
        {
            "linear" => Spread::Linear,
            "triangular" => Spread::Triangular,
            name => bail!("unsupported spread type {name}"),
        };
        Ok(Self {
            spacing,
            separation,
            spread,
            rules: PlacementRules::decode(value)?,
        })
    }
    /// The cell's deterministic candidate, prior to frequency and exclusions.
    pub fn potential_chunk(&self, seed: i64, source: [i32; 2]) -> [i32; 2] {
        let [x, z] = source;
        let gx = x.div_euclid(self.spacing);
        let gz = z.div_euclid(self.spacing);
        let mut random = salted(seed, gx, gz, self.rules.salt);
        let limit = self.spacing - self.separation;
        let mut offset = || {
            let a = random.next_int_bound(limit).unwrap();
            if self.spread == Spread::Linear {
                a
            } else {
                (a + random.next_int_bound(limit).unwrap()) / 2
            }
        };
        [
            gx.wrapping_mul(self.spacing).wrapping_add(offset()),
            gz.wrapping_mul(self.spacing).wrapping_add(offset()),
        ]
    }
    pub fn passes_frequency(&self, seed: i64, source: [i32; 2]) -> bool {
        self.rules.passes_frequency(seed, source)
    }
    pub fn is_candidate_before_exclusion(&self, seed: i64, source: [i32; 2]) -> bool {
        self.potential_chunk(seed, source) == source && self.passes_frequency(seed, source)
    }
    pub fn locate_position(&self, chunk: [i32; 2]) -> [i32; 3] {
        self.rules.locate_position(chunk)
    }
}

/// Java's concentric-ring placement. The returned positions are candidate
/// chunks; structure biome, start and piece validity are separate checks.
#[derive(Clone, Debug)]
pub struct ConcentricRingsPlacement {
    distance: i32,
    spread: i32,
    count: i32,
    preferred_biomes: Value,
    rules: PlacementRules,
}
impl ConcentricRingsPlacement {
    pub fn decode(value: &Value) -> Result<Self> {
        let kind = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing placement type")?,
        )?;
        ensure!(
            kind.namespace == "minecraft" && kind.path == "concentric_rings",
            "unsupported structure placement {}:{}",
            kind.namespace,
            kind.path
        );
        let distance = integer(value, "distance")?;
        let spread = integer(value, "spread")?;
        let count = integer(value, "count")?;
        ensure!(
            (0..=1023).contains(&distance)
                && (0..=1023).contains(&spread)
                && (1..=4095).contains(&count),
            "invalid concentric ring settings"
        );
        let preferred_biomes = value
            .get("preferred_biomes")
            .context("missing preferred biomes")?
            .clone();
        ensure!(
            preferred_biomes.is_string() || preferred_biomes.is_array(),
            "invalid preferred biome holder set"
        );
        if let Some(name) = preferred_biomes.as_str() {
            ResourceId::parse(name.strip_prefix('#').unwrap_or(name))?;
        } else if let Some(values) = preferred_biomes.as_array() {
            for value in values {
                let name = value.as_str().context("invalid preferred biome")?;
                ensure!(
                    !name.starts_with('#'),
                    "biome tags cannot appear inside a holder list"
                );
                ResourceId::parse(name)?;
            }
        }
        Ok(Self {
            distance,
            spread,
            count,
            preferred_biomes,
            rules: PlacementRules::decode(value)?,
        })
    }
    pub fn load(provider: &mut impl ResourceProvider, id: &ResourceId) -> Result<Self> {
        let path = format!(
            "data/{}/worldgen/structure_set/{}.json",
            id.namespace, id.path
        );
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing structure set {path}"))?;
        let value: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid structure set {path}"))?;
        Self::decode(
            value
                .get("placement")
                .context("missing structure placement")?,
        )
        .with_context(|| format!("in structure set {path}"))
    }
    pub fn preferred_biomes(&self) -> &Value {
        &self.preferred_biomes
    }
    pub fn locate_position(&self, chunk: [i32; 2]) -> [i32; 3] {
        self.rules.locate_position(chunk)
    }
    pub fn passes_frequency(&self, seed: i64, chunk: [i32; 2]) -> bool {
        self.rules.passes_frequency(seed, chunk)
    }

    /// Java forks one independent random source per ring search. A callback
    /// performs the biome search, returning None when no preferred biome exists.
    pub fn positions_with_search(
        &self,
        seed: i64,
        mut search: impl FnMut([i32; 2], &mut LegacyRandom) -> Result<Option<[i32; 2]>>,
    ) -> Result<Vec<[i32; 2]>> {
        let mut random = LegacyRandom::new(seed);
        let mut angle = random.next_double() * std::f64::consts::PI * 2.0;
        let mut spread = self.spread;
        let mut in_circle = 0;
        let mut circle = 0i32;
        let mut result = Vec::with_capacity(self.count as usize);
        for i in 0..self.count {
            let base = 4i32
                .wrapping_mul(self.distance)
                .wrapping_add(self.distance.wrapping_mul(circle).wrapping_mul(6));
            let distance =
                f64::from(base) + (random.next_double() - 0.5) * f64::from(self.distance) * 2.5;
            let round = |v: f64| -> i32 { (v + 0.5).floor() as i64 as i32 };
            let initial = [round(angle.cos() * distance), round(angle.sin() * distance)];
            let mut child = LegacyRandom::new(random.next_long());
            result.push(search(initial, &mut child)?.unwrap_or(initial));
            angle += std::f64::consts::PI * 2.0 / f64::from(spread);
            in_circle += 1;
            if in_circle == spread {
                circle += 1;
                in_circle = 0;
                spread += 2 * spread / (circle + 1);
                spread = spread.min(self.count - i);
                angle += random.next_double() * std::f64::consts::PI * 2.0;
            }
        }
        Ok(result)
    }
    /// Apply the inherited frequency and exclusion rules to precomputed rings.
    pub fn is_structure_chunk(
        &self,
        seed: i64,
        chunk: [i32; 2],
        positions: &[[i32; 2]],
        mut forbidden: impl FnMut(&ResourceId, [i32; 2], i32) -> Result<bool>,
    ) -> Result<bool> {
        if !positions.contains(&chunk) || !self.passes_frequency(seed, chunk) {
            return Ok(false);
        }
        if let Some((id, range)) = &self.rules.exclusion {
            if forbidden(id, chunk, *range)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Reproduce BiomeSource.findBiomeHorizontal's square quart scan and reservoir
/// choice for a stronghold ring. `preferred` receives [quart X, 0, quart Z].
pub fn find_ring_biome(
    initial: [i32; 2],
    random: &mut LegacyRandom,
    mut preferred: impl FnMut([i32; 3]) -> Result<bool>,
) -> Result<Option<[i32; 2]>> {
    let center = initial.map(|n| n.wrapping_mul(16).wrapping_add(8) >> 2);
    let mut found = 0i32;
    let mut selected = None;
    for z in -28i32..=28 {
        for x in -28i32..=28 {
            let qx = center[0].wrapping_add(x);
            let qz = center[1].wrapping_add(z);
            if preferred([qx, 0, qz])? {
                if selected.is_none() || random.next_int_bound(found + 1).unwrap() == 0 {
                    selected = Some([qx.wrapping_mul(4) >> 4, qz.wrapping_mul(4) >> 4]);
                }
                found += 1;
            }
        }
    }
    Ok(selected)
}

#[derive(Clone, Debug)]
pub struct StructureEntry {
    pub id: ResourceId,
    pub weight: i32,
}
#[derive(Clone, Debug)]
pub struct RandomSpreadSet {
    pub placement: RandomSpreadPlacement,
    pub structures: Vec<StructureEntry>,
}
/// Decode the weighted structure entries independently of placement type.
pub fn decode_structure_entries(bytes: &[u8]) -> Result<Vec<StructureEntry>> {
    let value: Value = serde_json::from_slice(bytes)?;
    entries(&value)
}
fn entries(value: &Value) -> Result<Vec<StructureEntry>> {
    let array = value
        .get("structures")
        .and_then(Value::as_array)
        .context("invalid structure list")?;
    let mut structures = Vec::with_capacity(array.len());
    let mut total = 0i32;
    for entry in array {
        let id = entry
            .get("structure")
            .and_then(Value::as_str)
            .context("inline or invalid structure is unsupported")?;
        let weight = integer(entry, "weight")?;
        ensure!(weight > 0, "invalid structure weight");
        total = total
            .checked_add(weight)
            .context("structure weight total overflow")?;
        structures.push(StructureEntry {
            id: ResourceId::parse(id)?,
            weight,
        });
    }
    Ok(structures)
}
impl RandomSpreadSet {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let value: Value = serde_json::from_slice(bytes)?;
        let placement = RandomSpreadPlacement::decode(
            value
                .get("placement")
                .context("missing structure placement")?,
        )?;
        let structures = entries(&value)?;
        Ok(Self {
            placement,
            structures,
        })
    }
    /// Order in which Java tries structures if preceding attempts fail.
    pub fn attempt_order(&self, seed: i64, chunk: [i32; 2]) -> Vec<ResourceId> {
        WeightedAttempts::new(&self.structures, seed, chunk)
            .map(|entry| entry.id)
            .collect()
    }
}

struct WeightedAttempts {
    options: Vec<StructureEntry>,
    total: i32,
    random: Option<LegacyRandom>,
}
impl WeightedAttempts {
    fn new(entries: &[StructureEntry], seed: i64, chunk: [i32; 2]) -> Self {
        Self {
            options: entries.to_vec(),
            total: entries.iter().map(|entry| entry.weight).sum(),
            random: (entries.len() >= 2).then(|| large_feature(seed, chunk[0], chunk[1])),
        }
    }
}
impl Iterator for WeightedAttempts {
    type Item = StructureEntry;
    fn next(&mut self) -> Option<Self::Item> {
        if self.options.is_empty() {
            return None;
        }
        let mut choice = self
            .random
            .as_mut()
            .map_or(0, |random| random.next_int_bound(self.total).unwrap());
        let mut index = 0;
        for v in &self.options {
            choice -= v.weight;
            if choice < 0 {
                break;
            }
            index += 1;
        }
        let chosen = self.options.remove(index);
        self.total -= chosen.weight;
        Some(chosen)
    }
}

/// Result of selecting and attempting entries in a structure set.
#[derive(Debug)]
pub enum StructureSetStart<T> {
    Existing,
    NotCandidate,
    Rejected,
    Generated { structure: ResourceId, start: T },
}

/// Cache belongs to one active data-pack stack and one generator configuration.
#[derive(Default)]
pub struct StructureSetCache {
    sets: BTreeMap<ResourceId, StoredSet>,
    rings: BTreeMap<(ResourceId, i64), Arc<[[i32; 2]]>>,
}
#[derive(Clone)]
enum StoredSet {
    Spread(RandomSpreadSet),
    Rings {
        placement: ConcentricRingsPlacement,
        structures: Vec<StructureEntry>,
    },
}
impl StoredSet {
    fn structures(&self) -> &[StructureEntry] {
        match self {
            Self::Spread(set) => &set.structures,
            Self::Rings { structures, .. } => structures,
        }
    }
}
fn missing_ring_positions<P: ResourceProvider>(
    _: &mut P,
    id: &ResourceId,
    _: &ConcentricRingsPlacement,
) -> Result<Vec<[i32; 2]>> {
    bail!(
        "concentric ring positions are required for {}:{}",
        id.namespace,
        id.path
    )
}
impl StructureSetCache {
    /// The cached ring list belongs to this cache's biome source and pack stack.
    pub fn cached_ring_positions(&self, id: &ResourceId, seed: i64) -> Option<&[[i32; 2]]> {
        self.rings.get(&(id.clone(), seed)).map(AsRef::as_ref)
    }
    fn load_set<'a>(
        &'a mut self,
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
    ) -> Result<&'a StoredSet> {
        if !self.sets.contains_key(id) {
            let path = format!(
                "data/{}/worldgen/structure_set/{}.json",
                id.namespace, id.path
            );
            let bytes = provider
                .read_data(&path)?
                .with_context(|| format!("missing structure set {path}"))?;
            let value: Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("invalid structure set {path}"))?;
            let placement = value
                .get("placement")
                .context("missing structure placement")?;
            let kind = ResourceId::parse(
                placement
                    .get("type")
                    .and_then(Value::as_str)
                    .context("missing placement type")?,
            )?;
            let set = match (kind.namespace.as_str(), kind.path.as_str()) {
                ("minecraft", "random_spread") => StoredSet::Spread(RandomSpreadSet {
                    placement: RandomSpreadPlacement::decode(placement)?,
                    structures: entries(&value)?,
                }),
                ("minecraft", "concentric_rings") => StoredSet::Rings {
                    placement: ConcentricRingsPlacement::decode(placement)?,
                    structures: entries(&value)?,
                },
                _ => bail!(
                    "unsupported structure placement {}:{}",
                    kind.namespace,
                    kind.path
                ),
            };
            self.sets.insert(id.clone(), set);
        }
        Ok(self.sets.get(id).unwrap())
    }
    /// Load a random-spread set. Ring sets require the ring-aware query.
    pub fn load<'a>(
        &'a mut self,
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
    ) -> Result<&'a RandomSpreadSet> {
        match self.load_set(provider, id)? {
            StoredSet::Spread(set) => Ok(set),
            StoredSet::Rings { .. } => bail!("structure set uses concentric rings"),
        }
    }
    pub fn is_structure_chunk<P: ResourceProvider>(
        &mut self,
        provider: &mut P,
        id: &ResourceId,
        seed: i64,
        chunk: [i32; 2],
    ) -> Result<bool> {
        self.resolve(
            provider,
            id,
            seed,
            chunk,
            &mut BTreeSet::new(),
            &mut missing_ring_positions::<P>,
        )
    }
    /// Compute ring positions on first use and retain them for this generator
    /// and seed. The resolver must use the same active biome source and packs.
    pub fn is_structure_chunk_with_rings<P: ResourceProvider>(
        &mut self,
        provider: &mut P,
        id: &ResourceId,
        seed: i64,
        chunk: [i32; 2],
        mut rings: impl FnMut(&mut P, &ResourceId, &ConcentricRingsPlacement) -> Result<Vec<[i32; 2]>>,
    ) -> Result<bool> {
        self.resolve(provider, id, seed, chunk, &mut BTreeSet::new(), &mut rings)
    }
    /// Apply the biome-source filter used before Java tests placement. The
    /// caller supplies the source's complete possible-biome set.
    pub fn is_structure_chunk_for_biomes(
        &mut self,
        provider: &mut impl ResourceProvider,
        biome_rules: &crate::structure_biomes::StructureBiomes,
        possible: &BTreeSet<ResourceId>,
        id: &ResourceId,
        seed: i64,
        chunk: [i32; 2],
    ) -> Result<bool> {
        if !biome_rules
            .set_has_possible_biome(self.load_set(provider, id)?.structures(), possible)?
        {
            return Ok(false);
        }
        self.is_structure_chunk(provider, id, seed, chunk)
    }
    pub fn is_structure_chunk_for_biomes_with_rings<P: ResourceProvider>(
        &mut self,
        provider: &mut P,
        biome_rules: &crate::structure_biomes::StructureBiomes,
        possible: &BTreeSet<ResourceId>,
        id: &ResourceId,
        seed: i64,
        chunk: [i32; 2],
        rings: impl FnMut(&mut P, &ResourceId, &ConcentricRingsPlacement) -> Result<Vec<[i32; 2]>>,
    ) -> Result<bool> {
        if !biome_rules
            .set_has_possible_biome(self.load_set(provider, id)?.structures(), possible)?
        {
            return Ok(false);
        }
        self.is_structure_chunk_with_rings(provider, id, seed, chunk, rings)
    }

    /// Attempt a set's entries in Java's weighted order, stopping at the first
    /// valid start. Errors are propagated rather than treated as invalid starts.
    /// The callbacks must use the same dimension, biome source and pack stack.
    #[allow(clippy::too_many_arguments)]
    pub fn try_generate_start<P: ResourceProvider, T>(
        &mut self,
        provider: &mut P,
        biome_rules: &crate::structure_biomes::StructureBiomes,
        possible: &BTreeSet<ResourceId>,
        id: &ResourceId,
        seed: i64,
        chunk: [i32; 2],
        mut existing: impl FnMut(&ResourceId) -> Result<bool>,
        mut generate: impl FnMut(&mut P, &StructureEntry) -> Result<Option<T>>,
        rings: impl FnMut(&mut P, &ResourceId, &ConcentricRingsPlacement) -> Result<Vec<[i32; 2]>>,
    ) -> Result<StructureSetStart<T>> {
        let structures = self.load_set(provider, id)?.structures().to_vec();
        if !biome_rules.set_has_possible_biome(&structures, possible)? {
            return Ok(StructureSetStart::NotCandidate);
        }
        for entry in &structures {
            if existing(&entry.id)? {
                return Ok(StructureSetStart::Existing);
            }
        }
        if !self.is_structure_chunk_with_rings(provider, id, seed, chunk, rings)? {
            return Ok(StructureSetStart::NotCandidate);
        }
        for entry in WeightedAttempts::new(&structures, seed, chunk) {
            if let Some(start) = generate(provider, &entry)? {
                return Ok(StructureSetStart::Generated {
                    structure: entry.id,
                    start,
                });
            }
        }
        Ok(StructureSetStart::Rejected)
    }
    fn resolve<
        P: ResourceProvider,
        F: FnMut(&mut P, &ResourceId, &ConcentricRingsPlacement) -> Result<Vec<[i32; 2]>>,
    >(
        &mut self,
        provider: &mut P,
        id: &ResourceId,
        seed: i64,
        chunk: [i32; 2],
        visiting: &mut BTreeSet<ResourceId>,
        rings: &mut F,
    ) -> Result<bool> {
        ensure!(
            visiting.len() < 128 && visiting.insert(id.clone()),
            "cyclic structure exclusion"
        );
        let result = (|| {
            let set = self.load_set(provider, id)?.clone();
            let rules = match set {
                StoredSet::Spread(set) => {
                    if !set.placement.is_candidate_before_exclusion(seed, chunk) {
                        return Ok(false);
                    }
                    set.placement.rules
                }
                StoredSet::Rings { placement, .. } => {
                    let key = (id.clone(), seed);
                    if !self.rings.contains_key(&key) {
                        let positions = rings(provider, id, &placement)?;
                        ensure!(
                            positions.len() == placement.count as usize,
                            "invalid concentric ring position count"
                        );
                        self.rings.insert(key.clone(), Arc::from(positions));
                    }
                    if !self.rings[&key].contains(&chunk)
                        || !placement.passes_frequency(seed, chunk)
                    {
                        return Ok(false);
                    }
                    placement.rules
                }
            };
            if let Some((other, range)) = rules.exclusion {
                for x in chunk[0].wrapping_sub(range)..=chunk[0].wrapping_add(range) {
                    for z in chunk[1].wrapping_sub(range)..=chunk[1].wrapping_add(range) {
                        if self.resolve(provider, &other, seed, [x, z], visiting, rings)? {
                            return Ok(false);
                        }
                    }
                }
            }
            Ok(true)
        })();
        visiting.remove(id);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[derive(Default)]
    struct Data(BTreeMap<String, Vec<u8>>);
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
    }
    fn set() -> Value {
        json!({"placement":{"type":"random_spread","spacing":1,"separation":0,"salt":0},"structures":[{"structure":"minecraft:test","weight":1}]})
    }
    #[test]
    fn rejects_invalid_settings_and_cyclic_exclusions() {
        let mut value = set();
        value["placement"]["spacing"] = json!(0);
        assert!(RandomSpreadSet::decode(&serde_json::to_vec(&value).unwrap()).is_err());
        value = set();
        value["placement"]["locate_offset"] = json!([17, 0, 0]);
        assert!(RandomSpreadSet::decode(&serde_json::to_vec(&value).unwrap()).is_err());
        value = set();
        value["structures"] = json!([{"structure":"minecraft:test","weight":2147483647},{"structure":"minecraft:test","weight":1}]);
        assert!(RandomSpreadSet::decode(&serde_json::to_vec(&value).unwrap()).is_err());
        let mut a = set();
        a["placement"]["exclusion_zone"] = json!({"other_set":"test:b","chunk_count":1});
        let mut b = set();
        b["placement"]["exclusion_zone"] = json!({"other_set":"test:a","chunk_count":1});
        let mut data = Data(BTreeMap::from([
            (
                "data/test/worldgen/structure_set/a.json".into(),
                serde_json::to_vec(&a).unwrap(),
            ),
            (
                "data/test/worldgen/structure_set/b.json".into(),
                serde_json::to_vec(&b).unwrap(),
            ),
        ]));
        let mut cache = StructureSetCache::default();
        assert!(
            cache
                .is_structure_chunk(&mut data, &ResourceId::parse("test:a").unwrap(), 0, [0, 0])
                .is_err()
        );
        assert!(
            cache
                .load(&mut data, &ResourceId::parse("test:missing").unwrap())
                .is_err()
        );
    }
    #[test]
    fn rings_validate_data_and_apply_frequency_and_exclusion_in_order() {
        let mut v = json!({"type":"concentric_rings","distance":32,"spread":3,"count":1,"salt":0,"preferred_biomes":"minecraft:plains","exclusion_zone":{"other_set":"test:other","chunk_count":2}});
        let placement = ConcentricRingsPlacement::decode(&v).unwrap();
        let mut invoked = 0;
        assert!(
            !placement
                .is_structure_chunk(0, [1, 2], &[[0, 0]], |_, _, _| {
                    invoked += 1;
                    Ok(false)
                })
                .unwrap()
        );
        assert_eq!(invoked, 0);
        assert!(
            !placement
                .is_structure_chunk(0, [0, 0], &[[0, 0]], |id, pos, range| {
                    assert_eq!(id.path, "other");
                    assert_eq!(pos, [0, 0]);
                    assert_eq!(range, 2);
                    Ok(true)
                })
                .unwrap()
        );
        v["frequency"] = json!(0);
        let never = ConcentricRingsPlacement::decode(&v).unwrap();
        assert!(
            !never
                .is_structure_chunk(0, [0, 0], &[[0, 0]], |_, _, _| panic!(
                    "frequency must be checked first"
                ))
                .unwrap()
        );
        v["count"] = json!(0);
        assert!(ConcentricRingsPlacement::decode(&v).is_err());
        let mut random = LegacyRandom::new(42);
        let mut untouched = random.clone();
        let selected =
            find_ring_biome([0, 0], &mut random, |[x, _, z]| Ok(x == -26 && z == -26)).unwrap();
        assert_eq!(selected, Some([-7, -7]));
        assert_eq!(random.next_long(), untouched.next_long());
    }
    #[test]
    fn cache_reuses_ring_positions_and_checks_cross_type_exclusions() {
        let rings = json!({"placement":{"type":"concentric_rings","distance":32,"spread":3,"count":1,"salt":0,"preferred_biomes":"minecraft:plains"},"structures":[{"structure":"minecraft:test","weight":1}]});
        let mut spread = set();
        spread["placement"]["exclusion_zone"] = json!({"other_set":"test:rings","chunk_count":1});
        let mut data = Data(BTreeMap::from([
            (
                "data/test/worldgen/structure_set/rings.json".into(),
                serde_json::to_vec(&rings).unwrap(),
            ),
            (
                "data/test/worldgen/structure_set/spread.json".into(),
                serde_json::to_vec(&spread).unwrap(),
            ),
        ]));
        let mut cache = StructureSetCache::default();
        let ring_id = ResourceId::parse("test:rings").unwrap();
        let spread_id = ResourceId::parse("test:spread").unwrap();
        assert!(
            cache
                .is_structure_chunk(&mut data, &ring_id, 42, [5, 6])
                .is_err()
        );
        let mut calls = 0;
        assert!(
            cache
                .is_structure_chunk_with_rings(&mut data, &ring_id, 42, [5, 6], |_, _, _| {
                    calls += 1;
                    Ok(vec![[5, 6]])
                })
                .unwrap()
        );
        assert!(
            !cache
                .is_structure_chunk_with_rings(&mut data, &ring_id, 42, [7, 6], |_, _, _| panic!(
                    "cached ring positions should be used"
                ))
                .unwrap()
        );
        assert!(
            !cache
                .is_structure_chunk_with_rings(&mut data, &spread_id, 42, [5, 5], |_, _, _| panic!(
                    "exclusion should use cached rings"
                ))
                .unwrap()
        );
        assert!(
            cache
                .is_structure_chunk_with_rings(&mut data, &ring_id, 43, [5, 6], |_, _, _| {
                    calls += 1;
                    Ok(vec![[5, 6]])
                })
                .unwrap()
        );
        assert_eq!(calls, 2);
        assert!(
            cache
                .is_structure_chunk_with_rings(&mut data, &ring_id, 44, [5, 6], |_, _, _| Ok(
                    vec![]
                ))
                .is_err()
        );
    }
}
