//! The effect library: every effect by name, plus loading effects from `.vfx.json` files.
//!
//! Built-in effects ([`crate::builtin`]) are always there. Games add their own in code
//! ([`EffectLibrary::insert`]), from files (`assets/effects/*.vfx.json`, hot reloaded), or
//! at runtime through the `vfx.define` API, which is how an AI agent designs a new effect
//! while the game runs. Replacing an effect restarts every instance that plays it.

use std::collections::HashMap;
use std::sync::Arc;

use engine_assets::{Asset, AssetLoader, Assets, LoadContext};
use engine_core::prelude::*;

use crate::effect::EffectDesc;

/// Every effect by name.
#[derive(Resource, Debug, Clone, Default)]
pub struct EffectLibrary {
    effects: HashMap<String, (Arc<EffectDesc>, u64)>,
    version: u64,
}

impl EffectLibrary {
    /// A library with the built-in effects.
    pub fn with_builtins() -> Self {
        let mut lib = Self::default();
        for desc in crate::builtin::all() {
            if let Err(e) = lib.insert(desc) {
                panic!("built-in effect is invalid: {e}");
            }
        }
        lib
    }

    /// Add or replace an effect (validated first).
    pub fn insert(&mut self, desc: EffectDesc) -> Result<(), String> {
        desc.validate()?;
        self.version += 1;
        self.effects
            .insert(desc.name.clone(), (Arc::new(desc), self.version));
        Ok(())
    }

    /// Remove an effect. Instances playing it stop and fade out.
    pub fn remove(&mut self, name: &str) -> Option<Arc<EffectDesc>> {
        self.effects.remove(name).map(|(d, _)| d)
    }

    pub fn get(&self, name: &str) -> Option<&Arc<EffectDesc>> {
        self.effects.get(name).map(|(d, _)| d)
    }

    /// The effect and the version it was last changed at.
    pub fn get_versioned(&self, name: &str) -> Option<(&Arc<EffectDesc>, u64)> {
        self.effects.get(name).map(|(d, v)| (d, *v))
    }

    pub fn contains(&self, name: &str) -> bool {
        self.effects.contains_key(name)
    }

    /// Effect names, sorted.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.effects.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    pub fn len(&self) -> usize {
        self.effects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }
}

/// Effects loaded from one `.vfx.json` file (one effect object, or a list of them).
#[derive(Debug, Clone)]
pub struct EffectFile {
    pub effects: Vec<EffectDesc>,
}

impl Asset for EffectFile {}

/// Parse a `.vfx.json` document: one effect or a list of effects.
pub fn parse_effects(text: &str) -> Result<Vec<EffectDesc>, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let effects: Vec<EffectDesc> = match value {
        serde_json::Value::Array(_) => serde_json::from_value(value),
        _ => serde_json::from_value(value).map(|e| vec![e]),
    }
    .map_err(|e| format!("not a valid effect: {e}"))?;
    for e in &effects {
        e.validate()?;
    }
    Ok(effects)
}

/// Loads `*.vfx.json` effect files.
pub struct EffectLoader;

impl AssetLoader for EffectLoader {
    fn extensions(&self) -> &[&str] {
        &["vfx.json"]
    }

    fn load(&self, bytes: &[u8], ctx: &mut LoadContext) -> Result<(), String> {
        let text =
            std::str::from_utf8(bytes).map_err(|_| "the file is not UTF-8 text".to_string())?;
        let effects = parse_effects(text)?;
        ctx.add_primary(EffectFile { effects });
        Ok(())
    }
}

/// Copy loaded (and hot-reloaded) effect files into the library.
pub fn sync_effect_files(
    files: Option<Res<Assets<EffectFile>>>,
    mut library: ResMut<EffectLibrary>,
    mut seen: Local<HashMap<String, u64>>,
) {
    let Some(files) = files else {
        return;
    };
    for (key, handle) in files.keys() {
        let version = files.version(handle);
        if seen.get(key) == Some(&version) {
            continue;
        }
        seen.insert(key.to_string(), version);
        let Some(file) = files.get(handle) else {
            continue;
        };
        for effect in &file.effects {
            if let Err(e) = library.insert(effect.clone()) {
                engine_core::log::warn!("vfx: {key}: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_valid_and_named() {
        let lib = EffectLibrary::with_builtins();
        assert!(lib.len() >= 20, "{}", lib.len());
        for name in [
            "spell.fireball",
            "spell.fireball_impact",
            "water.splash",
            "impact.sparks",
            "impact.dirt",
            "step.snow",
        ] {
            assert!(lib.contains(name), "{name} missing");
        }
    }

    #[test]
    fn replacing_bumps_the_version() {
        let mut lib = EffectLibrary::with_builtins();
        let (_, v1) = lib.get_versioned("spell.fireball").unwrap();
        let desc = (**lib.get("spell.fireball").unwrap()).clone();
        lib.insert(desc).unwrap();
        let (_, v2) = lib.get_versioned("spell.fireball").unwrap();
        assert!(v2 > v1);
    }

    #[test]
    fn files_hold_one_effect_or_a_list() {
        let one = r#"{"name": "a", "emitters": [{"rate": 5}]}"#;
        assert_eq!(parse_effects(one).unwrap().len(), 1);
        let list = r#"[{"name": "a", "emitters": [{"rate": 5}]}, {"name": "b", "emitters": [{"rate": 1}]}]"#;
        assert_eq!(parse_effects(list).unwrap().len(), 2);
        let bad = r#"{"name": "a", "emitters": [{"rate": 5, "lifetime": [0, 1]}]}"#;
        assert!(parse_effects(bad).unwrap_err().contains("lifetime"));
        assert!(parse_effects("{").unwrap_err().contains("JSON"));
    }
}
