//! Ruined-portal piece metadata. Block placement is not yet implemented.
use anyhow::{Context, Result, ensure};
use native_core::beardifier::BoundingBox;
use native_persistence::nbt::Tag;
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

impl PreparedStart {
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
