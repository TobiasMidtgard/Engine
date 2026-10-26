//! [`Scalability`]: one quality setting every module scales itself by.
//!
//! A game picks a [`QualityPreset`] (from `--quality low|medium|high|ultra`, a settings menu,
//! or an AI agent through the registry) and each module maps the factors to its own knobs:
//! the renderer to resolution, shadow map size and distance, volumetrics and SSGI; foliage
//! and particles to density; streaming to view distance; simulations to
//! [`SimulationRange`](crate::SimulationRange), which follows [`Scalability::simulation_m`]
//! automatically. Factors are relative to `High` (1.0), the look the showcase is tuned for.

use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};

/// The named presets, from fastest to finest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum QualityPreset {
    Low,
    Medium,
    #[default]
    High,
    Ultra,
}

impl QualityPreset {
    pub const ALL: [QualityPreset; 4] = [Self::Low, Self::Medium, Self::High, Self::Ultra];

    /// `"low"`, `"medium"`/`"med"`, `"high"` or `"ultra"`, any case.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" | "med" => Some(Self::Medium),
            "high" => Some(Self::High),
            "ultra" | "epic" => Some(Self::Ultra),
            _ => None,
        }
    }
}

/// Quality factors. Set [`preset`](Self::preset) with [`Scalability::preset`] to fill them
/// all, then override single fields if needed.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scalability {
    pub preset: QualityPreset,
    /// Internal render resolution relative to the window (upscaled to it), 0.5..1.
    pub render_scale: f32,
    /// Shadow quality: map size and shadow distance scale with it.
    pub shadows: f32,
    /// How far detail, LODs and streamed cells reach.
    pub view_distance: f32,
    /// Instance density of grass, debris, crowds and other small detail.
    pub density: f32,
    /// Effects quality: volumetrics, screen-space GI and reflections, particle counts.
    pub effects: f32,
    /// Meters around the camera that simulate in full (drives `SimulationRange`).
    pub simulation_m: f64,
}

impl Default for Scalability {
    fn default() -> Self {
        Self::preset(QualityPreset::High)
    }
}

impl Scalability {
    pub fn preset(preset: QualityPreset) -> Self {
        let (render_scale, shadows, view_distance, density, effects, simulation_m) = match preset {
            QualityPreset::Low => (0.67, 0.5, 0.5, 0.35, 0.5, 80.0),
            QualityPreset::Medium => (0.83, 0.75, 0.75, 0.6, 0.75, 120.0),
            QualityPreset::High => (1.0, 1.0, 1.0, 1.0, 1.0, 150.0),
            QualityPreset::Ultra => (1.0, 1.5, 1.5, 1.4, 1.25, 220.0),
        };
        Self {
            preset,
            render_scale,
            shadows,
            view_distance,
            density,
            effects,
            simulation_m,
        }
    }

    /// Whether an optional effect is on at this quality: on when `effects` is at least `min`.
    /// For example volumetric fog at 0.75, screen-space GI at 1.0.
    pub fn allows(&self, min: f32) -> bool {
        self.effects >= min
    }
}

/// `PreUpdate`: when [`Scalability`] changes, [`SimulationRange`](crate::SimulationRange)
/// follows its `simulation_m`.
pub fn apply_scalability(scalability: Res<Scalability>, mut range: ResMut<crate::SimulationRange>) {
    if scalability.is_changed() {
        range.active_m = scalability.simulation_m.max(1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_rise_from_low_to_ultra() {
        let all = QualityPreset::ALL.map(Scalability::preset);
        for pair in all.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            assert!(a.render_scale <= b.render_scale);
            assert!(a.shadows < b.shadows && a.density < b.density);
            assert!(a.view_distance < b.view_distance && a.simulation_m < b.simulation_m);
        }
        assert_eq!(Scalability::default().preset, QualityPreset::High);
        assert_eq!(Scalability::default().effects, 1.0);
        assert!(Scalability::preset(QualityPreset::Low).allows(0.5));
        assert!(!Scalability::preset(QualityPreset::Low).allows(0.75));
    }

    #[test]
    fn presets_parse_by_name() {
        assert_eq!(QualityPreset::parse("LOW"), Some(QualityPreset::Low));
        assert_eq!(QualityPreset::parse("med"), Some(QualityPreset::Medium));
        assert_eq!(QualityPreset::parse("ultra"), Some(QualityPreset::Ultra));
        assert_eq!(QualityPreset::parse("potato"), None);
    }
}
