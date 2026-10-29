use std::{
    collections::BTreeSet,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use zip::{ZipArchive, result::ZipError};

use crate::open_verified_client;

const MAX_RESOURCE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ResourceId {
    pub namespace: String,
    pub path: String,
}

impl ResourceId {
    pub fn parse(input: &str) -> Result<Self> {
        let (namespace, path) = input.split_once(':').unwrap_or(("minecraft", input));
        // Java treats an empty namespace before ':' as the default namespace.
        let namespace = if namespace.is_empty() {
            "minecraft"
        } else {
            namespace
        };
        ensure!(
            !namespace.is_empty()
                && namespace
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.-".contains(&c)),
            "invalid resource namespace: {namespace}"
        );
        ensure!(
            !path.is_empty()
                && path
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_./-".contains(&c)),
            "invalid resource path: {path}"
        );
        ensure!(
            path.split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."),
            "resource path contains an invalid component"
        );
        Ok(Self {
            namespace: namespace.to_owned(),
            path: path.to_owned(),
        })
    }

    pub fn asset_path(&self, kind: &str, extension: &str) -> String {
        format!(
            "assets/{}/{}/{}.{}",
            self.namespace, kind, self.path, extension
        )
    }
}

pub trait ResourceProvider {
    fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>>;

    /// All visible versions of an asset, from the built-in pack to the highest priority.
    fn read_asset_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
        Ok(self.read_asset(path)?.into_iter().collect())
    }

    /// Read built-in game data from the verified client, without consulting resource packs.
    /// This does not apply world data packs or represent the world's active registry.
    fn read_builtin_data(&mut self, _path: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    /// Read a resource from the selected world data packs.
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        self.read_builtin_data(path)
    }

    /// List visible JSON resource IDs in a data folder, excluding its folder prefix.
    fn list_data(&mut self, _folder: &str) -> Result<Vec<ResourceId>> {
        bail!("this resource provider cannot enumerate data")
    }

    /// All visible versions of one resource, in increasing pack priority.
    fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
        Ok(self.read_data(path)?.into_iter().collect())
    }

    fn read_asset_with_metadata(&mut self, path: &str) -> Result<Option<ResourceAsset>> {
        let Some(bytes) = self.read_asset(path)? else {
            return Ok(None);
        };
        let metadata = self.read_asset(&format!("{path}.mcmeta"))?;
        Ok(Some(ResourceAsset { bytes, metadata }))
    }
}

pub struct ResourceAsset {
    pub bytes: Vec<u8>,
    pub metadata: Option<Vec<u8>>,
}

pub(crate) enum Pack {
    Directory(PathBuf),
    Archive(ZipArchive<File>),
}

pub struct ResourceStack {
    base: ZipArchive<File>,
    packs: Vec<Pack>,
    world_data: Option<crate::datapacks::DataPackStack>,
}

impl ResourceStack {
    pub fn open(minecraft_dir: &Path) -> Result<Self> {
        Ok(Self {
            base: open_verified_client(minecraft_dir)?,
            packs: Vec::new(),
            world_data: None,
        })
    }

    /// Packs added later take priority over earlier packs and the vanilla client.
    pub fn push_pack(&mut self, path: &Path) -> Result<()> {
        let pack = if path.is_dir() {
            Pack::Directory(
                path.canonicalize()
                    .with_context(|| format!("cannot open resource pack {}", path.display()))?,
            )
        } else {
            let file = File::open(path)
                .with_context(|| format!("cannot open resource pack {}", path.display()))?;
            Pack::Archive(
                ZipArchive::new(file)
                    .with_context(|| format!("invalid resource pack ZIP {}", path.display()))?,
            )
        };
        self.packs.push(pack);
        Ok(())
    }

    /// Read only the highest-priority user pack containing an asset.
    /// This is useful when vanilla supplies its copy through the launcher asset index.
    pub fn read_pack_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        validate_asset_path(path)?;
        for pack in self.packs.iter_mut().rev() {
            if let Some(bytes) = pack.read(path)? {
                return Ok(Some(bytes));
            }
        }
        Ok(None)
    }

    /// List visible PNG texture identifiers below a texture directory across
    /// all namespaces and packs. Reads still resolve the highest-priority pack.
    pub fn list_textures(&self, folder: &str) -> Result<BTreeSet<ResourceId>> {
        let probe = if folder.is_empty() {
            "assets/minecraft/textures/entry.png".to_owned()
        } else {
            format!("assets/minecraft/textures/{folder}/entry.png")
        };
        validate_asset_path(&probe)?;
        let mut out = BTreeSet::new();
        collect_zip_textures(&self.base, folder, &mut out)?;
        for pack in &self.packs {
            match pack {
                Pack::Archive(archive) => collect_zip_textures(archive, folder, &mut out)?,
                Pack::Directory(root) => collect_directory_textures(root, folder, &mut out)?,
            }
        }
        Ok(out)
    }

    pub fn configure_world_data(
        &mut self,
        world_dir: &Path,
        settings: &crate::datapacks::DataPackSettings,
    ) -> Result<Vec<String>> {
        let (stack, warnings) =
            crate::datapacks::DataPackStack::open(world_dir, settings, &mut self.base)?;
        self.world_data = Some(stack);
        Ok(warnings)
    }

    pub fn selected_data_pack_ids(&self) -> Option<Vec<&str>> {
        self.world_data.as_ref().map(|stack| stack.selected_ids())
    }

    /// Block identifiers in the verified 26.3 client. Item frame blockstate
    /// assets describe entities and are not entries in the block registry.
    /// This is intentionally read from the installed JAR at runtime.
    pub fn vanilla_block_ids(&self) -> Result<BTreeSet<String>> {
        let mut ids = BTreeSet::new();
        for name in self.base.file_names() {
            let Some(name) = name
                .strip_prefix("assets/minecraft/blockstates/")
                .and_then(|name| name.strip_suffix(".json"))
            else {
                continue;
            };
            if name.contains('/') || matches!(name, "item_frame" | "glow_item_frame") {
                continue;
            }
            let id = ResourceId::parse(name)?;
            ids.insert(format!("{}:{}", id.namespace, id.path));
        }
        ensure!(!ids.is_empty(), "verified client has no blockstate assets");
        Ok(ids)
    }

    /// Entity type inheritance is read from the verified client itself. Packs
    /// cannot override the Java classes that define collision behavior.
    pub fn vanilla_entity_collision_registry(
        &mut self,
    ) -> Result<crate::entity_collision::EntityCollisionRegistry> {
        crate::entity_collision::EntityCollisionRegistry::load(|path| {
            read_zip(&mut self.base, path)
        })
    }

    /// Read registration code only from the hash-verified client JAR.
    pub fn vanilla_item_registry(&mut self) -> Result<crate::item_registry::ItemRegistry> {
        crate::item_registry::ItemRegistry::load(|path| read_zip(&mut self.base, path))
    }
}

fn collect_texture_name(name: &str, folder: &str, out: &mut BTreeSet<ResourceId>) -> Result<()> {
    let Some(rest) = name.strip_prefix("assets/") else {
        return Ok(());
    };
    let Some((namespace, rest)) = rest.split_once('/') else {
        return Ok(());
    };
    let Some(path) = rest
        .strip_prefix("textures/")
        .and_then(|path| path.strip_suffix(".png"))
    else {
        return Ok(());
    };
    if !folder.is_empty()
        && !path
            .strip_prefix(folder)
            .is_some_and(|suffix| suffix.starts_with('/'))
    {
        return Ok(());
    }
    if let Ok(id) = ResourceId::parse(&format!("{namespace}:{path}")) {
        out.insert(id);
        ensure!(out.len() <= 100_000, "too many texture resources");
    }
    Ok(())
}

fn collect_zip_textures(
    archive: &ZipArchive<File>,
    folder: &str,
    out: &mut BTreeSet<ResourceId>,
) -> Result<()> {
    for name in archive.file_names() {
        collect_texture_name(name, folder, out)?;
    }
    Ok(())
}

fn collect_directory_textures(
    root: &Path,
    folder: &str,
    out: &mut BTreeSet<ResourceId>,
) -> Result<()> {
    let assets = root.join("assets");
    let assets = match assets.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("cannot inspect resource pack assets"),
    };
    ensure!(
        assets.starts_with(root),
        "resource pack assets escape their root"
    );
    if !assets.is_dir() {
        return Ok(());
    }
    for namespace in std::fs::read_dir(&assets)? {
        let namespace = namespace?;
        let Some(name) = namespace.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if ResourceId::parse(&format!("{name}:entry")).is_err() {
            continue;
        }
        let target = namespace.path().join("textures").join(folder);
        let resource = if folder.is_empty() {
            format!("assets/{name}/textures")
        } else {
            format!("assets/{name}/textures/{folder}")
        };
        collect_texture_tree(
            root,
            &target,
            &resource,
            folder,
            0,
            &mut BTreeSet::new(),
            out,
        )?;
    }
    Ok(())
}

fn collect_texture_tree(
    root: &Path,
    path: &Path,
    resource: &str,
    folder: &str,
    depth: usize,
    ancestors: &mut BTreeSet<PathBuf>,
    out: &mut BTreeSet<ResourceId>,
) -> Result<()> {
    ensure!(depth <= 64, "resource pack texture nesting is too deep");
    let canonical = match path.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("cannot inspect resource pack texture directory"),
    };
    ensure!(
        canonical.starts_with(root),
        "resource pack texture escapes its root"
    );
    if !canonical.is_dir() {
        return Ok(());
    }
    ensure!(
        ancestors.insert(canonical.clone()),
        "resource pack texture directory contains a cycle"
    );
    for entry in std::fs::read_dir(&canonical)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let resource = format!("{resource}/{name}");
        let target = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() || kind.is_symlink() && target.is_dir() {
            collect_texture_tree(root, &target, &resource, folder, depth + 1, ancestors, out)?;
        } else if kind.is_file() || kind.is_symlink() && target.is_file() {
            ensure!(
                target.canonicalize()?.starts_with(root),
                "resource pack texture escapes its root"
            );
            collect_texture_name(&resource, folder, out)?;
        }
    }
    ancestors.remove(&canonical);
    Ok(())
}

impl ResourceProvider for ResourceStack {
    fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        validate_asset_path(path)?;
        for pack in self.packs.iter_mut().rev() {
            let value = pack.read(path)?;
            if value.is_some() {
                return Ok(value);
            }
        }
        read_zip(&mut self.base, path)
    }

    fn read_asset_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
        validate_asset_path(path)?;
        let mut sources: Vec<_> = read_zip(&mut self.base, path)?.into_iter().collect();
        for pack in &mut self.packs {
            if let Some(bytes) = pack.read(path)? {
                sources.push(bytes);
            }
        }
        Ok(sources)
    }

    fn read_asset_with_metadata(&mut self, path: &str) -> Result<Option<ResourceAsset>> {
        validate_asset_path(path)?;
        let metadata_path = format!("{path}.mcmeta");
        let mut metadata = None;
        // Metadata from higher-priority packs may override a resource, but metadata from
        // packs below the selected resource must not be applied to the replacement image.
        for pack in self.packs.iter_mut().rev() {
            if metadata.is_none() {
                metadata = pack.read(&metadata_path)?;
            }
            if let Some(bytes) = pack.read(path)? {
                return Ok(Some(ResourceAsset { bytes, metadata }));
            }
        }
        if metadata.is_none() {
            metadata = read_zip(&mut self.base, &metadata_path)?;
        }
        Ok(read_zip(&mut self.base, path)?.map(|bytes| ResourceAsset { bytes, metadata }))
    }

    fn read_builtin_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        validate_path(path, "data/")?;
        read_zip(&mut self.base, path)
    }

    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        validate_path(path, "data/")?;
        if let Some(world_data) = &mut self.world_data {
            world_data.read(&mut self.base, path)
        } else {
            read_zip(&mut self.base, path)
        }
    }

    fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
        validate_data_folder(folder)?;
        let ids = if let Some(world_data) = &mut self.world_data {
            world_data.list(&self.base, folder)?
        } else {
            let mut ids = BTreeSet::new();
            collect_zip_data(&self.base, "", folder, &mut ids)?;
            ids
        };
        Ok(ids.into_iter().collect())
    }

    fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
        validate_path(path, "data/")?;
        if let Some(world_data) = &mut self.world_data {
            world_data.read_stack(&mut self.base, path)
        } else {
            Ok(read_zip(&mut self.base, path)?.into_iter().collect())
        }
    }
}

impl Pack {
    pub(crate) fn read(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        match self {
            Self::Directory(root) => read_directory(root, path),
            Self::Archive(archive) => read_zip(archive, path),
        }
    }
}

fn validate_asset_path(path: &str) -> Result<()> {
    validate_path(path, "assets/")
}

pub(crate) fn validate_path(path: &str, prefix: &str) -> Result<()> {
    ensure!(
        path.starts_with(prefix)
            && path
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..")
            && path
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_./-".contains(&c)),
        "invalid resource path"
    );
    Ok(())
}

fn validate_data_folder(folder: &str) -> Result<()> {
    validate_path(&format!("data/minecraft/{folder}/entry.json"), "data/")
}

pub(crate) fn collect_zip_data(
    archive: &ZipArchive<File>,
    prefix: &str,
    folder: &str,
    out: &mut BTreeSet<ResourceId>,
) -> Result<()> {
    let root = if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}/")
    };
    for name in archive.file_names() {
        if let Some(name) = name.strip_prefix(&root) {
            collect_data_name(name, folder, out)?;
        }
    }
    Ok(())
}

pub(crate) fn collect_pack_data(
    pack: &Pack,
    prefix: &str,
    folder: &str,
    out: &mut BTreeSet<ResourceId>,
) -> Result<()> {
    match pack {
        Pack::Archive(archive) => collect_zip_data(archive, prefix, folder, out),
        Pack::Directory(root) => collect_directory_data(root, prefix, folder, out),
    }
}

fn collect_data_name(name: &str, folder: &str, out: &mut BTreeSet<ResourceId>) -> Result<()> {
    let Some(rest) = name.strip_prefix("data/") else {
        return Ok(());
    };
    let Some((namespace, rest)) = rest.split_once('/') else {
        return Ok(());
    };
    let Some(path) = rest
        .strip_prefix(folder)
        .and_then(|s| s.strip_prefix('/'))
        .and_then(|s| s.strip_suffix(".json"))
    else {
        return Ok(());
    };
    if let Ok(id) = ResourceId::parse(&format!("{namespace}:{path}")) {
        out.insert(id);
        ensure!(out.len() <= 100_000, "too many data resources in {folder}");
    }
    Ok(())
}

fn collect_directory_data(
    root: &Path,
    prefix: &str,
    folder: &str,
    out: &mut BTreeSet<ResourceId>,
) -> Result<()> {
    let data_root = root.join(prefix).join("data");
    let data_root = match data_root.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("cannot inspect data pack directory"),
    };
    ensure!(
        data_root.starts_with(root),
        "data pack directory escapes its root"
    );
    if !data_root.is_dir() {
        return Ok(());
    }
    for namespace in std::fs::read_dir(&data_root)? {
        let namespace = namespace?;
        let Some(name) = namespace.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if ResourceId::parse(&format!("{name}:entry")).is_err() {
            continue;
        }
        let target = namespace.path().join(folder);
        let mut ancestors = BTreeSet::new();
        collect_directory_tree(
            root,
            &target,
            &format!("data/{name}/{folder}"),
            folder,
            0,
            &mut ancestors,
            out,
        )?;
    }
    Ok(())
}

fn collect_directory_tree(
    root: &Path,
    path: &Path,
    resource: &str,
    folder: &str,
    depth: usize,
    ancestors: &mut BTreeSet<PathBuf>,
    out: &mut BTreeSet<ResourceId>,
) -> Result<()> {
    ensure!(depth <= 64, "data pack resource nesting is too deep");
    let canonical = match path.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("cannot inspect data pack resource directory"),
    };
    ensure!(
        canonical.starts_with(root),
        "data pack resource escapes its root"
    );
    if !canonical.is_dir() {
        return Ok(());
    }
    ensure!(
        ancestors.insert(canonical.clone()),
        "data pack resource directory contains a cycle"
    );
    for entry in std::fs::read_dir(&canonical)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let resource = format!("{resource}/{name}");
        let target = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() || kind.is_symlink() && target.is_dir() {
            collect_directory_tree(root, &target, &resource, folder, depth + 1, ancestors, out)?;
        } else if kind.is_file() || kind.is_symlink() && target.is_file() {
            let resolved = target.canonicalize()?;
            ensure!(
                resolved.starts_with(root),
                "data pack resource escapes its root"
            );
            collect_data_name(&resource, folder, out)?;
        }
    }
    ancestors.remove(&canonical);
    Ok(())
}

fn read_directory(root: &Path, asset: &str) -> Result<Option<Vec<u8>>> {
    let path = root.join(asset);
    let canonical = match path.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", path.display())),
    };
    ensure!(
        canonical.starts_with(root),
        "resource pack asset escapes its root"
    );
    let file =
        File::open(&canonical).with_context(|| format!("cannot read {}", canonical.display()))?;
    read_limited(file, asset).map(Some)
}

pub(crate) fn read_zip(archive: &mut ZipArchive<File>, asset: &str) -> Result<Option<Vec<u8>>> {
    let entry = match archive.by_name(asset) {
        Ok(entry) => entry,
        Err(ZipError::FileNotFound) => return Ok(None),
        Err(error) => bail!("cannot read resource {asset}: {error}"),
    };
    ensure!(
        entry.size() <= MAX_RESOURCE_BYTES,
        "resource {asset} is too large"
    );
    read_limited(entry, asset).map(Some)
}

fn read_limited(reader: impl Read, asset: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_RESOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("cannot read resource {asset}"))?;
    ensure!(
        bytes.len() as u64 <= MAX_RESOURCE_BYTES,
        "resource {asset} is too large"
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_namespaced_and_cannot_escape_asset_roots() {
        assert_eq!(
            ResourceId::parse("block/stone").unwrap().namespace,
            "minecraft"
        );
        assert_eq!(
            ResourceId::parse(":block/stone").unwrap(),
            ResourceId::parse("minecraft:block/stone").unwrap()
        );
        assert_eq!(
            ResourceId::parse("example:block/test").unwrap().path,
            "block/test"
        );
        for invalid in [
            "../secret",
            "minecraft:/etc",
            "UPPER:block",
            "minecraft:block/../secret",
            "minecraft:block\\file",
        ] {
            assert!(ResourceId::parse(invalid).is_err());
        }
    }

    #[test]
    fn directory_pack_cannot_read_outside_its_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("assets/minecraft/models/block")).unwrap();
        std::fs::write(
            root.path().join("assets/minecraft/models/block/a.json"),
            b"sample",
        )
        .unwrap();
        let canonical = root.path().canonicalize().unwrap();
        assert_eq!(
            read_directory(&canonical, "assets/minecraft/models/block/a.json").unwrap(),
            Some(b"sample".to_vec())
        );
        assert!(
            read_directory(&canonical, "assets/missing.json")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn texture_directory_enumeration_respects_namespace_and_folder_boundaries() {
        let root = tempfile::tempdir().unwrap();
        for name in [
            "assets/minecraft/textures/item/a.png",
            "assets/minecraft/textures/item/nested/b.png",
            "assets/extra/textures/item/c.png",
            "assets/minecraft/textures/item/a.png.mcmeta",
            "assets/minecraft/textures/items/unrelated.png",
            "assets/minecraft/models/item/not_texture.png",
        ] {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"placeholder").unwrap();
        }
        let mut found = BTreeSet::new();
        collect_directory_textures(&root.path().canonicalize().unwrap(), "item", &mut found)
            .unwrap();
        assert_eq!(
            found,
            [
                "minecraft:item/a",
                "minecraft:item/nested/b",
                "extra:item/c"
            ]
            .into_iter()
            .map(|name| ResourceId::parse(name).unwrap())
            .collect()
        );
    }
}
