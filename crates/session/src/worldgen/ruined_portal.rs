//! Ruined-portal piece metadata and staged template placement.
use anyhow::{Context, Result, ensure};
use native_core::beardifier::BoundingBox;
use native_persistence::nbt::{Tag, TagType};
use native_persistence::structure_template::StructureTemplate;
use native_resources::{
    packs::{ResourceId, ResourceProvider},
    ruined_portal::{
        GenerationPoint, InitialSelection, PortalEnvironment, Settings, Setup, VerticalPlacement,
    },
    structure_start::decode_terrain_adjustment,
    structure_transform::{Mirror, Rotation, bounding_box},
};
use std::sync::Arc;

/// Access to the selected generation biome's temperature at the portal origin.
pub trait PortalStartEnvironment: PortalEnvironment {
    fn cold_enough_to_snow(
        &mut self,
        provider: &mut impl ResourceProvider,
        position: [i32; 3],
    ) -> Result<bool>;
}

/// A validated start and its unmodified template, prior to decoration.
#[derive(Clone)]
pub struct PreparedStart {
    pub chunk: [i32; 2],
    pub point: GenerationPoint,
    pub terrain_adjustment: native_core::beardifier::TerrainAdjustment,
    pub template: Arc<StructureTemplate>,
    pub piece: SavedRuinedPortalPiece,
}

/// A jigsaw marker from the original template, resolved for its later pass.
#[derive(Clone, Debug, PartialEq)]
pub struct PortalJigsawMarker {
    pub source_index: usize,
    pub position: [i32; 3],
    pub final_state: native_world::loaded::BlockState,
}

// Java clips template entities using their transformed integer blockPos,
// independently of their floating-point Pos. Reject records whose location
// cannot be established instead of silently omitting a possible entity.
fn template_entities_intersect(
    entities: &[Tag],
    origin: [i32; 3],
    mirror: Mirror,
    rotation: Rotation,
    pivot: [i32; 3],
    clip: BoundingBox,
) -> Result<bool> {
    ensure!(
        (0..3).all(|axis| clip.min[axis] <= clip.max[axis]),
        "invalid portal clipping box"
    );
    for entity in entities {
        let Tag::Compound(fields) = entity else {
            anyhow::bail!("invalid portal template entity");
        };
        let mut positions = fields.iter().filter(|(name, _)| name.matches("blockPos"));
        let (_, position) = positions.next().context("portal entity blockPos missing")?;
        ensure!(
            positions.next().is_none(),
            "duplicate portal entity blockPos"
        );
        let local = match position {
            Tag::IntArray(values) => match values.as_slice() {
                [x, y, z] => [*x, *y, *z],
                _ => anyhow::bail!("invalid portal entity blockPos length"),
            },
            Tag::List {
                element_type: TagType::Int,
                values,
            } => match values.as_slice() {
                [Tag::Int(x), Tag::Int(y), Tag::Int(z)] => [*x, *y, *z],
                _ => anyhow::bail!("invalid portal entity blockPos"),
            },
            _ => anyhow::bail!("invalid portal entity blockPos"),
        };
        let world = native_resources::structure_transform::block_world_position(
            local, origin, mirror, rotation, pivot,
        );
        if (0..3).all(|axis| (clip.min[axis]..=clip.max[axis]).contains(&world[axis])) {
            return Ok(true);
        }
    }
    Ok(false)
}

impl PreparedStart {
    fn selected_palette_index(&self) -> Result<usize> {
        let [x, y, z] = self.piece.position;
        let count =
            i32::try_from(self.template.palettes.len()).context("too many portal palettes")?;
        Ok(
            native_core::random::LegacyRandom::new(native_resources::blockstates::positional_seed(
                x, y, z,
            ))
            .next_int_bound(count)
            .context("portal template has no palettes")? as usize,
        )
    }

    /// Java filters the original selected palette after placing the template,
    /// independently of processors. NBT-bearing entries are ordered by local
    /// Y, X, Z and clipped after their mirror and rotation are applied.
    pub fn jigsaw_markers(
        &self,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
        clip: BoundingBox,
    ) -> Result<Vec<PortalJigsawMarker>> {
        self.piece.validate()?;
        ensure!(
            self.template.size == self.piece.template_size,
            "portal template size changed"
        );
        ensure!(
            (0..3).all(|axis| clip.min[axis] <= clip.max[axis]),
            "invalid portal clip"
        );
        let palette = &self.template.palettes[self.selected_palette_index()?];
        let pivot = [self.template.size[0] / 2, 0, self.template.size[2] / 2];
        let mut markers = Vec::new();
        for (source_index, block) in self.template.blocks.iter().enumerate() {
            let Some(Tag::String(name)) = palette
                .get(block.state)
                .context("portal palette index out of bounds")?
                .get("id")
            else {
                anyhow::bail!("template block identifier missing")
            };
            if !(name.matches("minecraft:jigsaw") || name.matches("jigsaw")) {
                continue;
            }
            let Some(nbt) = &block.nbt else { continue };
            let Tag::Compound(fields) = nbt else {
                anyhow::bail!("jigsaw marker is not a compound")
            };
            let position = native_resources::structure_transform::block_world_position(
                block.position,
                self.piece.position,
                self.piece.mirror,
                self.piece.rotation,
                pivot,
            );
            if !(0..3).all(|axis| (clip.min[axis]..=clip.max[axis]).contains(&position[axis])) {
                continue;
            }
            ensure!(
                fields
                    .iter()
                    .filter(|(key, _)| key.matches("final_state"))
                    .count()
                    <= 1,
                "duplicate jigsaw final_state"
            );
            let raw = match nbt.get("final_state") {
                Some(Tag::String(value)) => value.to_string_lossy(),
                _ => "minecraft:air".to_owned(),
            };
            let final_state =
                native_persistence::jigsaw_replacement::parse_final_state(registry, &raw)?
                    .unwrap_or_else(|| native_world::loaded::BlockState {
                        name: "minecraft:air".into(),
                        properties: Default::default(),
                    });
            markers.push((
                block.position,
                PortalJigsawMarker {
                    source_index,
                    position,
                    final_state,
                },
            ));
        }
        markers.sort_by_key(|(position, _)| (position[1], position[0], position[2]));
        Ok(markers.into_iter().map(|(_, marker)| marker).collect())
    }

    /// Place this template in an unpublished world and finalize jigsaw markers
    /// after the ordinary shape passes. Entities inside the clip remain unsupported.
    /// The caller discards its world draft on any failure.
    pub fn place_template<W: super::jigsaw::StructurePlacementWorld>(
        &self,
        world: &mut W,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
        processors: &native_persistence::structure_processors::PreparedProcessors,
        clip: BoundingBox,
        is_static_full: impl FnMut(&native_world::loaded::BlockState) -> Result<bool>,
        random: &mut native_core::random::DecorationRandom,
    ) -> Result<bool> {
        self.piece.validate()?;
        ensure!(
            self.template.size == self.piece.template_size,
            "portal template size changed"
        );
        if self.template.palettes.is_empty()
            || self.template.size.iter().any(|&size| size < 1)
            || (self.template.blocks.is_empty() && self.template.entities.is_empty())
        {
            return Ok(false);
        }
        ensure!(
            !template_entities_intersect(
                &self.template.entities,
                self.piece.position,
                self.piece.mirror,
                self.piece.rotation,
                [self.template.size[0] / 2, 0, self.template.size[2] / 2],
                clip,
            )?,
            "portal template entity placement is unsupported"
        );
        let markers = self.jigsaw_markers(registry, clip)?;
        let selected = self.selected_palette_index()?;
        let palette = &self.template.palettes[selected];
        let pivot = [self.template.size[0] / 2, 0, self.template.size[2] / 2];
        for block in &self.template.blocks {
            let name = match palette
                .get(block.state)
                .context("portal palette index out of bounds")?
                .get("id")
            {
                Some(Tag::String(name)) => name,
                _ => anyhow::bail!("template block identifier missing"),
            };
            let position = native_resources::structure_transform::block_world_position(
                block.position,
                self.piece.position,
                self.piece.mirror,
                self.piece.rotation,
                pivot,
            );
            if !(0..3).all(|axis| (clip.min[axis]..=clip.max[axis]).contains(&position[axis])) {
                continue;
            }
            if (name.matches("minecraft:structure_block") || name.matches("structure_block"))
                && let Some(nbt) = &block.nbt
            {
                let Some(Tag::String(mode)) = nbt.get("mode") else {
                    anyhow::bail!("portal marker mode missing")
                };
                ensure!(
                    ["SAVE", "LOAD", "CORNER", "DATA"]
                        .iter()
                        .any(|value| mode.matches(value)),
                    "invalid portal marker mode"
                );
            }
        }
        let shared = std::cell::RefCell::new(world);
        let records = self.process_blocks(
            registry,
            processors,
            clip,
            is_static_full,
            &mut |position| {
                super::jigsaw::JigsawPlacementWorld::block(&mut **shared.borrow_mut(), position)
            },
        )?;
        let world = shared.into_inner();
        let mut next_random = random.clone();
        super::jigsaw::place_structure_records(
            &records,
            world,
            registry,
            clip,
            self.piece.mirror,
            self.piece.rotation,
            &mut next_random,
        )?;
        for marker in markers {
            world.finalize_jigsaw_marker(marker.position, &marker.final_state)?;
        }
        world.finish_jigsaw_markers()?;
        *random = next_random;
        Ok(true)
    }

    /// Process template records in the piece's mirror, rotation, and pivot.
    /// State transforms, block placement, and decoration happen afterwards.
    pub fn process_blocks(
        &self,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
        processors: &native_persistence::structure_processors::PreparedProcessors,
        clip: BoundingBox,
        is_static_full: impl FnMut(&native_world::loaded::BlockState) -> Result<bool>,
        world: &mut impl FnMut([i32; 3]) -> Result<native_world::loaded::BlockState>,
    ) -> Result<Vec<native_persistence::jigsaw_template::ProcessedTemplateBlock>> {
        self.piece.validate()?;
        ensure!(
            self.template.size == self.piece.template_size,
            "portal template size changed"
        );
        native_persistence::jigsaw_template::process_structure_template(
            &self.template,
            processors,
            native_persistence::jigsaw_template::StructureTemplateSettings {
                origin: self.piece.position,
                mirror: self.piece.mirror,
                rotation: self.piece.rotation,
                pivot: [self.template.size[0] / 2, 0, self.template.size[2] / 2],
                clip,
            },
            |tag| native_persistence::jigsaw_template::resolve_template_state(tag, registry),
            is_static_full,
            world,
        )
    }

    /// Match Java's initial selection, height and final biome check.
    /// Block placement and world saving remain separate operations.
    pub fn prepare(
        provider: &mut impl ResourceProvider,
        definition: &serde_json::Value,
        environment: &mut impl PortalStartEnvironment,
        seed: i64,
        chunk: [i32; 2],
    ) -> Result<Option<Self>> {
        let settings = Settings::decode(definition)?;
        let terrain_adjustment = decode_terrain_adjustment(definition)?;
        let (selection, mut random) = settings.select_for_chunk(seed, chunk)?;
        let template = StructureTemplate::load(provider, &selection.template)?
            .context("ruined portal template is missing")?;
        ensure!(
            template.data_version == Some(5023),
            "ruined portal template requires data-version conversion"
        );
        let setup = &settings.setups()[selection.setup_index];
        let Some(point) = native_resources::ruined_portal::find_generation_point(
            environment,
            chunk,
            template.size,
            &selection,
            setup,
            &mut random,
        )?
        else {
            return Ok(None);
        };
        let cold = if setup.can_be_cold {
            environment.cold_enough_to_snow(provider, point.position)?
        } else {
            false
        };
        let piece =
            SavedRuinedPortalPiece::capture(&selection, setup, &point, template.size, cold)?;
        Ok(Some(Self {
            chunk,
            point,
            terrain_adjustment,
            template: Arc::new(template),
            piece,
        }))
    }

    pub fn reference_bounds(
        &self,
        structure: ResourceId,
    ) -> Result<super::structure_references::StructureStartBounds> {
        super::structure_references::StructureStartBounds::from_pieces(
            structure,
            &[self.piece.bounds],
            self.terrain_adjustment,
        )
    }

    /// Serialize a fresh, single-piece start without authorizing a world save.
    pub fn to_tag(&self, structure: &ResourceId) -> Result<Tag> {
        let name = format!("{}:{}", structure.namespace, structure.path);
        ensure!(
            ResourceId::parse(&name)? == *structure,
            "invalid portal structure identifier"
        );
        let x = self.chunk[0]
            .checked_mul(16)
            .context("portal chunk X overflow")?;
        let z = self.chunk[1]
            .checked_mul(16)
            .context("portal chunk Z overflow")?;
        ensure!(
            self.template.data_version == Some(5023)
                && self.template.size == self.piece.template_size
                && self.point.position == self.piece.position
                && self.point.position[0] == x
                && self.point.position[2] == z
                && self.point.bounds == self.piece.bounds
                && self.point.pivot == [self.template.size[0] / 2, 0, self.template.size[2] / 2],
            "portal start differs from its piece or template"
        );
        Ok(Tag::Compound(vec![
            ("id".into(), Tag::String(name.as_str().into())),
            ("ChunkX".into(), Tag::Int(self.chunk[0])),
            ("ChunkZ".into(), Tag::Int(self.chunk[1])),
            ("references".into(), Tag::Int(0)),
            (
                "Children".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![self.piece.to_tag()?],
                },
            ),
        ]))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Properties {
    pub cold: bool,
    pub mossiness: f32,
    pub air_pocket: bool,
    pub overgrown: bool,
    pub vines: bool,
    pub replace_with_blackstone: bool,
}

/// One piece whose template dimensions were resolved from the active pack.
/// This alone does not authorize saving a generated world.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedRuinedPortalPiece {
    pub template: ResourceId,
    pub position: [i32; 3],
    pub rotation: Rotation,
    pub mirror: Mirror,
    pub placement: VerticalPlacement,
    pub properties: Properties,
    pub bounds: BoundingBox,
    template_size: [i32; 3],
}

impl SavedRuinedPortalPiece {
    pub(super) fn template_size(&self) -> [i32; 3] {
        self.template_size
    }

    /// Decorate an unpublished region after its template has been placed.
    pub fn decorate(
        &self,
        world: &mut impl native_resources::ruined_portal_decoration::PortalDecorationWorld,
        random: &mut native_core::random::DecorationRandom,
    ) -> Result<()> {
        self.validate()?;
        native_resources::ruined_portal_decoration::PortalDecoration {
            bounds: self.bounds,
            placement: self.placement,
            cold: self.properties.cold,
            overgrown: self.properties.overgrown,
            vines: self.properties.vines,
        }
        .apply(world, random)
    }

    /// Prepare the template processor chain from this piece's saved settings.
    pub fn prepare_processors(
        &self,
        provider: &mut impl ResourceProvider,
        blocks: &std::collections::BTreeSet<String>,
    ) -> Result<native_persistence::structure_processors::PreparedProcessors> {
        self.validate()?;
        native_persistence::structure_processors::PreparedProcessors::ruined_portal(
            provider,
            blocks,
            self.placement,
            self.properties.air_pocket,
            self.properties.cold,
            self.properties.mossiness,
            self.properties.replace_with_blackstone,
        )
    }

    pub fn capture(
        selection: &InitialSelection,
        setup: &Setup,
        point: &GenerationPoint,
        template_size: [i32; 3],
        cold: bool,
    ) -> Result<Self> {
        ensure!(!cold || setup.can_be_cold, "portal setup cannot be cold");
        let piece = Self {
            template: selection.template.clone(),
            position: point.position,
            rotation: selection.rotation,
            mirror: selection.mirror,
            placement: setup.placement,
            properties: Properties {
                cold,
                mossiness: setup.mossiness,
                air_pocket: selection.air_pocket,
                overgrown: setup.overgrown,
                vines: setup.vines,
                replace_with_blackstone: setup.replace_with_blackstone,
            },
            bounds: point.bounds,
            template_size,
        };
        ensure!(
            point.pivot == [template_size[0] / 2, 0, template_size[2] / 2],
            "invalid portal pivot"
        );
        piece.validate()?;
        Ok(piece)
    }

    fn validate(&self) -> Result<()> {
        let id = ResourceId::parse(&format!(
            "{}:{}",
            self.template.namespace, self.template.path
        ))?;
        ensure!(id == self.template, "invalid portal template identifier");
        ensure!(
            self.template_size.iter().all(|&value| value > 0),
            "invalid portal template size"
        );
        ensure!(
            self.properties.mossiness.is_finite(),
            "invalid portal mossiness"
        );
        let pivot = [self.template_size[0] / 2, 0, self.template_size[2] / 2];
        ensure!(
            self.bounds
                == bounding_box(
                    self.template_size,
                    self.position,
                    self.mirror,
                    self.rotation,
                    pivot
                )?,
            "portal bounds differ from template position"
        );
        Ok(())
    }

    pub fn to_tag(&self) -> Result<Tag> {
        self.validate()?;
        let rotation = match self.rotation {
            Rotation::None => "NONE",
            Rotation::Clockwise90 => "CLOCKWISE_90",
            Rotation::Clockwise180 => "CLOCKWISE_180",
            Rotation::Counterclockwise90 => "COUNTERCLOCKWISE_90",
        };
        let mirror = match self.mirror {
            Mirror::None => "NONE",
            Mirror::LeftRight => "LEFT_RIGHT",
            Mirror::FrontBack => "FRONT_BACK",
        };
        let placement = match self.placement {
            VerticalPlacement::OnLandSurface => "on_land_surface",
            VerticalPlacement::PartlyBuried => "partly_buried",
            VerticalPlacement::OnOceanFloor => "on_ocean_floor",
            VerticalPlacement::InMountain => "in_mountain",
            VerticalPlacement::Underground => "underground",
            VerticalPlacement::InNether => "in_nether",
        };
        let p = &self.properties;
        let [x, y, z] = self.position;
        Ok(Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:rupo".into())),
            (
                "BB".into(),
                Tag::IntArray(self.bounds.min.into_iter().chain(self.bounds.max).collect()),
            ),
            ("O".into(), Tag::Int(2)),
            ("GD".into(), Tag::Int(0)),
            ("TPX".into(), Tag::Int(x)),
            ("TPY".into(), Tag::Int(y)),
            ("TPZ".into(), Tag::Int(z)),
            (
                "Template".into(),
                Tag::String(
                    format!("{}:{}", self.template.namespace, self.template.path)
                        .as_str()
                        .into(),
                ),
            ),
            ("Rotation".into(), Tag::String(rotation.into())),
            ("Mirror".into(), Tag::String(mirror.into())),
            ("VerticalPlacement".into(), Tag::String(placement.into())),
            (
                "Properties".into(),
                Tag::Compound(vec![
                    ("cold".into(), Tag::Byte(i8::from(p.cold))),
                    ("mossiness".into(), Tag::Float(p.mossiness)),
                    ("air_pocket".into(), Tag::Byte(i8::from(p.air_pocket))),
                    ("overgrown".into(), Tag::Byte(i8::from(p.overgrown))),
                    ("vines".into(), Tag::Byte(i8::from(p.vines))),
                    (
                        "replace_with_blackstone".into(),
                        Tag::Byte(i8::from(p.replace_with_blackstone)),
                    ),
                ]),
            ),
        ]))
    }

    /// Reject data that cannot be preserved exactly, including unknown fields.
    pub fn from_tag(tag: &Tag, template_size: [i32; 3]) -> Result<Self> {
        fn fields(tag: &Tag, names: &[&str]) -> Result<()> {
            let Tag::Compound(fields) = tag else {
                anyhow::bail!("portal piece is not a compound")
            };
            ensure!(
                fields.len() == names.len()
                    && names.iter().all(|name| fields
                        .iter()
                        .filter(|(key, _)| key.matches(name))
                        .count()
                        == 1),
                "portal piece has unknown, missing, or duplicate fields"
            );
            Ok(())
        }
        fields(
            tag,
            &[
                "id",
                "BB",
                "O",
                "GD",
                "TPX",
                "TPY",
                "TPZ",
                "Template",
                "Rotation",
                "Mirror",
                "VerticalPlacement",
                "Properties",
            ],
        )?;
        let text = |name: &str| -> Result<String> {
            match tag.get(name) {
                Some(Tag::String(value)) => String::from_utf16(&value.0)
                    .with_context(|| format!("invalid portal field {name}")),
                _ => anyhow::bail!("invalid portal field {name}"),
            }
        };
        let integer = |name: &str| -> Result<i32> {
            match tag.get(name) {
                Some(Tag::Int(value)) => Ok(*value),
                _ => anyhow::bail!("invalid portal field {name}"),
            }
        };
        ensure!(
            text("id")? == "minecraft:rupo" && integer("O")? == 2 && integer("GD")? == 0,
            "unsupported portal piece identity or orientation"
        );
        let rotation = match text("Rotation")?.as_str() {
            "NONE" => Rotation::None,
            "CLOCKWISE_90" => Rotation::Clockwise90,
            "CLOCKWISE_180" => Rotation::Clockwise180,
            "COUNTERCLOCKWISE_90" => Rotation::Counterclockwise90,
            _ => anyhow::bail!("invalid portal rotation"),
        };
        let mirror = match text("Mirror")?.as_str() {
            "NONE" => Mirror::None,
            "LEFT_RIGHT" => Mirror::LeftRight,
            "FRONT_BACK" => Mirror::FrontBack,
            _ => anyhow::bail!("invalid portal mirror"),
        };
        let placement = match text("VerticalPlacement")?.as_str() {
            "on_land_surface" => VerticalPlacement::OnLandSurface,
            "partly_buried" => VerticalPlacement::PartlyBuried,
            "on_ocean_floor" => VerticalPlacement::OnOceanFloor,
            "in_mountain" => VerticalPlacement::InMountain,
            "underground" => VerticalPlacement::Underground,
            "in_nether" => VerticalPlacement::InNether,
            _ => anyhow::bail!("invalid portal vertical placement"),
        };
        let props = tag.get("Properties").context("portal properties missing")?;
        fields(
            props,
            &[
                "cold",
                "mossiness",
                "air_pocket",
                "overgrown",
                "vines",
                "replace_with_blackstone",
            ],
        )?;
        let boolean = |name: &str| -> Result<bool> {
            match props.get(name) {
                Some(Tag::Byte(0)) => Ok(false),
                Some(Tag::Byte(1)) => Ok(true),
                _ => anyhow::bail!("invalid portal property {name}"),
            }
        };
        let mossiness = match props.get("mossiness") {
            Some(Tag::Float(value)) => *value,
            _ => anyhow::bail!("invalid portal mossiness"),
        };
        let properties = Properties {
            cold: boolean("cold")?,
            mossiness,
            air_pocket: boolean("air_pocket")?,
            overgrown: boolean("overgrown")?,
            vines: boolean("vines")?,
            replace_with_blackstone: boolean("replace_with_blackstone")?,
        };
        let Some(Tag::IntArray(b)) = tag.get("BB") else {
            anyhow::bail!("invalid portal bounds")
        };
        ensure!(b.len() == 6, "invalid portal bounds length");
        let template_text = text("Template")?;
        let template = ResourceId::parse(&template_text)?;
        ensure!(
            format!("{}:{}", template.namespace, template.path) == template_text,
            "noncanonical portal template"
        );
        let piece = Self {
            template,
            position: [integer("TPX")?, integer("TPY")?, integer("TPZ")?],
            rotation,
            mirror,
            placement,
            properties,
            bounds: BoundingBox::new([b[0], b[1], b[2]], [b[3], b[4], b[5]])
                .map_err(anyhow::Error::msg)?,
            template_size,
        };
        piece.validate()?;
        Ok(piece)
    }
}

#[cfg(test)]
mod entity_clip_tests {
    use super::*;

    fn entity(position: Tag) -> Tag {
        Tag::Compound(vec![("blockPos".into(), position)])
    }

    #[test]
    fn template_entity_clipping_uses_transformed_integer_anchor() -> Result<()> {
        let list = Tag::List {
            element_type: TagType::Int,
            values: vec![Tag::Int(3), Tag::Int(4), Tag::Int(-2)],
        };
        let args = (
            [-32, 60, 48],
            Mirror::FrontBack,
            Rotation::Clockwise90,
            [2, 0, 3],
        );
        let point = BoundingBox::new([-25, 64, 46], [-25, 64, 46]).unwrap();
        let outside = BoundingBox::new([-24, 64, 46], [-24, 64, 46]).unwrap();
        for position in [list, Tag::IntArray(vec![3, 4, -2])] {
            let entities = [entity(position)];
            assert!(template_entities_intersect(
                &entities, args.0, args.1, args.2, args.3, point
            )?);
            assert!(!template_entities_intersect(
                &entities, args.0, args.1, args.2, args.3, outside
            )?);
        }
        assert!(!template_entities_intersect(
            &[],
            args.0,
            args.1,
            args.2,
            args.3,
            point
        )?);
        Ok(())
    }

    #[test]
    fn template_entity_clipping_rejects_unknown_anchors() {
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).unwrap();
        let cases = [
            Tag::Compound(vec![]),
            entity(Tag::IntArray(vec![1, 2])),
            entity(Tag::String("1 2 3".into())),
            Tag::Compound(vec![
                ("blockPos".into(), Tag::IntArray(vec![1, 2, 3])),
                ("blockPos".into(), Tag::IntArray(vec![30, 2, 3])),
            ]),
        ];
        for value in cases {
            assert!(
                template_entities_intersect(
                    &[value],
                    [0, 0, 0],
                    Mirror::None,
                    Rotation::None,
                    [0, 0, 0],
                    clip
                )
                .is_err()
            );
        }
    }
}
