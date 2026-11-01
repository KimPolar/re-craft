//! Release launch options that also have equivalents in the GUI.

#[cfg(any(not(debug_assertions), test))]
use std::ffi::OsString;
use std::path::PathBuf;

#[cfg(any(not(debug_assertions), test))]
use anyhow::{Context, Result, bail};

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LaunchOptions {
    pub minecraft_dir: Option<PathBuf>,
    pub resource_packs: Option<Vec<PathBuf>>,
}

impl LaunchOptions {
    #[cfg(any(not(debug_assertions), test))]
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let mut result = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--minecraft-dir" {
                if result.minecraft_dir.is_some() {
                    bail!("--minecraft-dir may be supplied only once");
                }
                let path = args.next().context("--minecraft-dir requires a path")?;
                if path.is_empty() || path.to_string_lossy().starts_with("--") {
                    bail!("--minecraft-dir requires a path");
                }
                result.minecraft_dir = Some(path.into());
            } else if arg == "--resource-pack" {
                let path = args.next().context("--resource-pack requires a path")?;
                if path.is_empty() || path.to_string_lossy().starts_with("--") {
                    bail!("--resource-pack requires a path");
                }
                result
                    .resource_packs
                    .get_or_insert_with(Vec::new)
                    .push(path.into());
            } else {
                bail!("Unknown launch option: {}", arg.to_string_lossy());
            }
        }
        Ok(result)
    }

    pub fn resolve(self, settings: &crate::menu_settings::Settings) -> (PathBuf, Vec<PathBuf>) {
        let changed_directory = self.minecraft_dir.is_some();
        let directory = self
            .minecraft_dir
            .or_else(|| settings.minecraft_dir.clone())
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap_or_default());
        let packs = self.resource_packs.unwrap_or_else(|| {
            if changed_directory {
                Vec::new()
            } else {
                settings.resource_packs.clone()
            }
        });
        (directory, packs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<LaunchOptions> {
        LaunchOptions::parse(args.iter().map(OsString::from))
    }

    #[test]
    fn only_gui_supported_options_are_accepted() {
        assert_eq!(parse(&[]).unwrap(), LaunchOptions::default());
        assert_eq!(
            parse(&[
                "--minecraft-dir",
                "C:\\게임\\.minecraft",
                "--resource-pack",
                "a.zip",
                "--resource-pack",
                "b"
            ])
            .unwrap(),
            LaunchOptions {
                minecraft_dir: Some("C:\\게임\\.minecraft".into()),
                resource_packs: Some(vec!["a.zip".into(), "b".into()])
            }
        );
        for args in [
            &["--minecraft-dir"][..],
            &["--resource-pack"],
            &["--minecraft-dir", ""],
            &["--minecraft-dir", "--smoke-test"],
            &["--resource-pack", "--resource-pack", "p"],
            &["--minecraft-dir", "a", "--minecraft-dir", "b"],
            &["--inspect-world", "world"],
            &["--smoke-test"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn overrides_follow_gui_directory_and_pack_selection() {
        let settings = crate::menu_settings::Settings {
            minecraft_dir: Some("old".into()),
            resource_packs: vec!["old-pack".into()],
            ..Default::default()
        };
        assert_eq!(
            parse(&[]).unwrap().resolve(&settings),
            ("old".into(), vec!["old-pack".into()])
        );
        assert_eq!(
            parse(&["--minecraft-dir", "new"])
                .unwrap()
                .resolve(&settings),
            ("new".into(), vec![])
        );
        assert_eq!(
            parse(&["--resource-pack", "a", "--resource-pack", "b"])
                .unwrap()
                .resolve(&settings),
            ("old".into(), vec!["a".into(), "b".into()])
        );
        assert_eq!(
            parse(&["--minecraft-dir", "new", "--resource-pack", "p"])
                .unwrap()
                .resolve(&settings),
            ("new".into(), vec!["p".into()])
        );
    }
}
