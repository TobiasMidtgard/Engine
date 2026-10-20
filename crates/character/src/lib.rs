#![doc = include_str!("../README.md")]

pub mod api;
pub mod appearance;
pub mod build;
pub mod demo;
pub mod eyes;
pub mod feature;
pub mod mesh;
pub mod morph;
pub mod pack;
pub mod sliders;
pub mod strands;
pub mod subdiv;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};

use engine_core::prelude::*;
use engine_render::RenderAppExt;

pub use appearance::{
    Ancestry, Appearance, Beard, BeardStyle, Brows, Eyes, Hair, HairColor, HairStyle, Skin,
};
pub use build::{BuildOptions, CharacterMeshes};
pub use feature::{CharacterFeature, GpuCharacter};
pub use mesh::Region;
pub use pack::HumanPack;

/// How a character is built: how much of the body, how smooth, and whether it has hair.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterQuality {
    /// `bust` (head, neck, shoulders) or `head`.
    pub region: RegionName,
    /// Skin subdivision levels, 0..=2 (2 for close-ups).
    pub levels: u32,
    /// Grow hair strands.
    pub hair: bool,
}

/// Serializable [`Region`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegionName {
    Head,
    #[default]
    Bust,
}

impl Default for CharacterQuality {
    fn default() -> Self {
        Self {
            region: RegionName::Bust,
            levels: 2,
            hair: true,
        }
    }
}

impl CharacterQuality {
    fn options(&self) -> BuildOptions {
        BuildOptions {
            region: match self.region {
                RegionName::Head => Region::Head,
                RegionName::Bust => Region::Bust,
            },
            levels: self.levels.min(2),
            hair: self.hair,
        }
    }
}

/// The meshes built for an entity's [`Appearance`] (replaced whenever it changes).
#[derive(Component, Clone)]
pub struct BuiltCharacter {
    pub meshes: Arc<CharacterMeshes>,
    /// Increases with every rebuild.
    pub version: u64,
    /// Colors and landmarks for the shaders.
    pub uniform: GpuCharacter,
    /// The appearance it was built from.
    pub appearance: Appearance,
}

/// A build running in the background for an entity.
#[derive(Component)]
struct PendingBuild {
    rx: Mutex<Receiver<(Appearance, CharacterMeshes)>>,
}

/// Debug views of the character shaders: 0 off, 1 region masks, 2 normals, 3 curvature.
#[derive(Resource, Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterDebug {
    pub view: u8,
}

/// Where the human pack stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackStatus {
    Loading,
    Ready,
    /// The pack couldn't be read; the message says what to do. Characters aren't drawn.
    Missing(String),
}

/// The human pack and its region masks, loaded in the background at startup.
#[derive(Resource)]
pub struct HumanAssets {
    pub pack: Option<Arc<HumanPack>>,
    pub regions: Option<Arc<mesh::Regions>>,
    pub status: PackStatus,
    rx: Mutex<Option<Receiver<Result<(HumanPack, mesh::Regions), String>>>>,
}

impl HumanAssets {
    /// Assets already in memory (tests, tools).
    pub fn ready(pack: HumanPack) -> Self {
        let regions = mesh::Regions::new(&pack);
        Self {
            pack: Some(Arc::new(pack)),
            regions: Some(Arc::new(regions)),
            status: PackStatus::Ready,
            rx: Mutex::new(None),
        }
    }

    fn loading(path: PathBuf) -> Self {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let result = std::fs::read(&path)
                .map_err(|e| {
                    format!(
                        "the human pack {} could not be read ({e}). Run `cargo run -p fetch-assets -- character` to download it.",
                        path.display()
                    )
                })
                .and_then(|bytes| HumanPack::parse(&bytes))
                .map(|pack| {
                    let regions = mesh::Regions::new(&pack);
                    (pack, regions)
                });
            let _ = tx.send(result);
        });
        Self {
            pack: None,
            regions: None,
            status: PackStatus::Loading,
            rx: Mutex::new(Some(rx)),
        }
    }
}

/// System sets of the character module (in `Update`).
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CharacterSet;

/// Characters with faces and bodies from a morphable human model (MakeHuman's CC0 base mesh
/// and shape targets), drawn with skin, eye and hair shading. Add an [`Appearance`] (and a
/// `Transform`) to an entity to give it a person; edit the appearance to change them.
pub struct CharacterPlugin;

impl Plugin for CharacterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CharacterDebug>()
            .register_component::<Appearance>(
                "A person's look: sex (0 female..1 male), age (years), muscle, weight, height, proportions (0..1), ancestry (african/asian/european weights), shape (face sliders by key, -1..1; see character.sliders), skin (melanin, redness, undertone, freckles, moles, oiliness, wrinkles, pores), eyes (melanin, lipochrome, limbal_ring, pupil, redness), hair (style Bald/Buzz/Short/Medium/Long/Bun, color {melanin, red, grey}, length, curl, recession), brows, beard (style None/Stubble/Short/Full/Goatee, density), seed.",
            )
            .register_component::<CharacterQuality>(
                "How a character is built: region (bust or head), levels (skin subdivision 0..2), hair (grow strands).",
            )
            .register_resource::<CharacterDebug>(
                "Character shader debug view: 0 off, 1 region masks, 2 normals, 3 curvature.",
            )
            .add_systems(Startup, load_pack)
            .add_systems(
                Update,
                (poll_pack, start_builds, finish_builds)
                    .chain()
                    .in_set(CharacterSet),
            )
            .add_render_feature(CharacterFeature::default());
        api::register(app);
    }
}

fn load_pack(world: &mut World) {
    if world.contains_resource::<HumanAssets>() {
        return;
    }
    let root = world
        .get_resource::<engine_assets::AssetServer>()
        .map(|s| s.root())
        .unwrap_or_else(|| PathBuf::from("assets"));
    world.insert_resource(HumanAssets::loading(root.join(pack::PACK_PATH)));
}

fn poll_pack(assets: Option<ResMut<HumanAssets>>) {
    let Some(mut assets) = assets else {
        return;
    };
    let result = {
        let rx = assets.rx.get_mut().unwrap_or_else(|e| e.into_inner());
        match rx.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(r)) => {
                *rx = None;
                Some(r)
            }
            Some(Err(TryRecvError::Disconnected)) => {
                *rx = None;
                Some(Err(
                    "the human pack loader stopped unexpectedly.".to_string()
                ))
            }
            _ => None,
        }
    };
    match result {
        Some(Ok((pack, regions))) => {
            tracing::info!(
                "character: human pack loaded ({} vertices, {} targets)",
                pack.positions.len(),
                pack.targets.len()
            );
            assets.pack = Some(Arc::new(pack));
            assets.regions = Some(Arc::new(regions));
            assets.status = PackStatus::Ready;
        }
        Some(Err(e)) => {
            tracing::warn!("character: {e} Characters are not drawn until then.");
            assets.status = PackStatus::Missing(e);
        }
        None => {}
    }
}

#[allow(clippy::type_complexity)]
fn start_builds(
    mut commands: Commands,
    assets: Option<Res<HumanAssets>>,
    q: Query<(
        Entity,
        &Appearance,
        Option<&CharacterQuality>,
        Option<&BuiltCharacter>,
        Has<PendingBuild>,
    )>,
) {
    let Some(assets) = assets else {
        return;
    };
    let (Some(pack), Some(regions)) = (assets.pack.clone(), assets.regions.clone()) else {
        return;
    };
    for (entity, appearance, quality, built, pending) in &q {
        if pending || built.is_some_and(|b| b.appearance == *appearance) {
            continue;
        }
        let a = appearance.clone();
        let opts = quality.copied().unwrap_or_default().options();
        let (pack, regions) = (pack.clone(), regions.clone());
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let meshes = build::build(&pack, &regions, &a, opts);
            let _ = tx.send((a, meshes));
        });
        commands
            .entity(entity)
            .insert(PendingBuild { rx: Mutex::new(rx) });
    }
}

fn finish_builds(
    mut commands: Commands,
    assets: Option<Res<HumanAssets>>,
    q: Query<(Entity, &PendingBuild, Option<&BuiltCharacter>)>,
) {
    let rest = assets
        .as_ref()
        .and_then(|a| a.regions.as_ref())
        .map(|r| r.landmarks);
    for (entity, pending, built) in &q {
        let msg = pending
            .rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_recv();
        match msg {
            Ok((appearance, meshes)) => {
                let Some(rest) = rest else {
                    continue;
                };
                let version = built.map_or(1, |b| b.version + 1);
                commands
                    .entity(entity)
                    .remove::<PendingBuild>()
                    .insert(BuiltCharacter {
                        meshes: Arc::new(meshes),
                        version,
                        uniform: GpuCharacter::new(&appearance, &rest, 0),
                        appearance,
                    });
            }
            Err(TryRecvError::Disconnected) => {
                tracing::warn!("character: a build stopped unexpectedly; retrying");
                commands.entity(entity).remove::<PendingBuild>();
            }
            Err(TryRecvError::Empty) => {}
        }
    }
}
