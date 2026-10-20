//! The human pack: MakeHuman's CC0 base mesh (hm08) and its shape targets, baked into one file
//! by `tools/bake_makehuman.py` (format in that script's docs) and fetched to
//! `assets/fetched/character/human_hm08.bin`.

use std::collections::HashMap;

use engine_core::math::{Vec2, Vec3};

/// Asset path of the pack under the asset root.
pub const PACK_PATH: &str = "fetched/character/human_hm08.bin";

/// One quad of the base mesh (triangles repeat their last corner).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad {
    pub v: [u32; 4],
    pub uv: [u32; 4],
    /// Index into [`HumanPack::groups`].
    pub group: u16,
}

/// A shape target: sparse vertex offsets (m) and the factors whose product weighs it.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// Path under MakeHuman's `targets/` without the extension (`nose/nose-hump-incr`).
    pub name: String,
    /// Macro values (`female`, `young`, `maxweight`, `african`, …) and slider halves
    /// (`nose-hump-incr`); see [`crate::morph`].
    pub factors: Vec<String>,
    pub indices: Vec<u32>,
    pub deltas: Vec<Vec3>,
}

/// The base mesh and its targets.
#[derive(Debug, Clone, Default)]
pub struct HumanPack {
    /// Rest positions, m (Y up, facing +Z, about 1.67 m tall, centered near the hips).
    pub positions: Vec<Vec3>,
    pub uvs: Vec<Vec2>,
    /// Face group names (`body`, `helper-l-eye`, `helper-upper-teeth`, `joint-head`, …).
    pub groups: Vec<String>,
    pub quads: Vec<Quad>,
    pub targets: Vec<Target>,
    by_name: HashMap<String, usize>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.bytes.len());
        let Some(end) = end else {
            return Err(format!(
                "the human pack ends early (needed {n} bytes at offset {}). Fetch it again with \
                 `cargo run -p fetch-assets -- character`.",
                self.at
            ));
        };
        let s = &self.bytes[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn i16(&mut self) -> Result<i16, String> {
        Ok(self.u16()? as i16)
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.u16()? as usize;
        String::from_utf8(self.take(n)?.to_vec())
            .map_err(|_| "the human pack has a name that isn't UTF-8.".to_string())
    }
    fn count(&mut self, what: &str, limit: usize) -> Result<usize, String> {
        let n = self.u32()? as usize;
        if n > limit {
            return Err(format!(
                "the human pack claims {n} {what}, more than {limit}; the file is damaged."
            ));
        }
        Ok(n)
    }
}

impl HumanPack {
    /// Parse a pack written by `tools/bake_makehuman.py`.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut r = Reader { bytes, at: 0 };
        if r.take(4)? != b"MHPK" {
            return Err("this is not a human pack (no MHPK header).".into());
        }
        let version = r.u32()?;
        if version != 1 {
            return Err(format!(
                "human pack version {version} is not supported (expected 1); update the engine or \
                 fetch the pack again."
            ));
        }
        let nv = r.count("vertices", 1 << 22)?;
        let mut positions = Vec::with_capacity(nv);
        for _ in 0..nv {
            positions.push(Vec3::new(r.f32()?, r.f32()?, r.f32()?));
        }
        let nuv = r.count("uvs", 1 << 22)?;
        let mut uvs = Vec::with_capacity(nuv);
        for _ in 0..nuv {
            uvs.push(Vec2::new(r.f32()?, r.f32()?));
        }
        let ng = r.count("groups", 1 << 16)?;
        let mut groups = Vec::with_capacity(ng);
        for _ in 0..ng {
            groups.push(r.string()?);
        }
        let nq = r.count("quads", 1 << 22)?;
        let mut quads = Vec::with_capacity(nq);
        for _ in 0..nq {
            let v = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
            let uv = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
            let group = r.u16()?;
            if v.iter().any(|i| *i as usize >= nv)
                || uv.iter().any(|i| *i as usize >= nuv)
                || group as usize >= ng
            {
                return Err("the human pack has a quad pointing past its vertices.".into());
            }
            quads.push(Quad { v, uv, group });
        }
        let nt = r.count("targets", 1 << 16)?;
        let mut targets = Vec::with_capacity(nt);
        for _ in 0..nt {
            let name = r.string()?;
            let nf = r.u8()? as usize;
            let mut factors = Vec::with_capacity(nf);
            for _ in 0..nf {
                factors.push(r.string()?);
            }
            let scale = r.f32()?;
            let n = r.count("target vertices", nv)?;
            let mut indices = Vec::with_capacity(n);
            for _ in 0..n {
                let i = r.u32()?;
                if i as usize >= nv {
                    return Err(format!(
                        "target {name} moves a vertex the mesh doesn't have."
                    ));
                }
                indices.push(i);
            }
            let mut deltas = Vec::with_capacity(n);
            for _ in 0..n {
                let d = [r.i16()?, r.i16()?, r.i16()?];
                deltas.push(Vec3::new(d[0] as f32, d[1] as f32, d[2] as f32) * scale);
            }
            targets.push(Target {
                name,
                factors,
                indices,
                deltas,
            });
        }
        Ok(Self::new(positions, uvs, groups, quads, targets))
    }

    /// A pack from parts (tests build small ones).
    pub fn new(
        positions: Vec<Vec3>,
        uvs: Vec<Vec2>,
        groups: Vec<String>,
        quads: Vec<Quad>,
        targets: Vec<Target>,
    ) -> Self {
        let by_name = targets
            .iter()
            .enumerate()
            .map(|(i, t)| (t.name.clone(), i))
            .collect();
        Self {
            positions,
            uvs,
            groups,
            quads,
            targets,
            by_name,
        }
    }

    /// The target with this name (`mouth/mouth-upperlip-volume-incr`).
    pub fn target(&self, name: &str) -> Option<&Target> {
        self.by_name.get(name).map(|i| &self.targets[*i])
    }

    /// Index of a face group.
    pub fn group(&self, name: &str) -> Option<u16> {
        self.groups.iter().position(|g| g == name).map(|i| i as u16)
    }

    /// Every vertex used by the quads of a group, sorted.
    pub fn group_vertices(&self, name: &str) -> Vec<u32> {
        let Some(g) = self.group(name) else {
            return Vec::new();
        };
        let mut out: Vec<u32> = self
            .quads
            .iter()
            .filter(|q| q.group == g)
            .flat_map(|q| q.v)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_str(out: &mut Vec<u8>, s: &str) {
        out.extend((s.len() as u16).to_le_bytes());
        out.extend(s.as_bytes());
    }

    /// A pack with one quad and one target, in the baked format.
    pub(crate) fn tiny_pack_bytes() -> Vec<u8> {
        let mut b = b"MHPK".to_vec();
        b.extend(1u32.to_le_bytes());
        b.extend(4u32.to_le_bytes());
        for p in [
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ] {
            for c in p {
                b.extend(c.to_le_bytes());
            }
        }
        b.extend(1u32.to_le_bytes());
        b.extend(0.5f32.to_le_bytes());
        b.extend(0.5f32.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        write_str(&mut b, "body");
        b.extend(1u32.to_le_bytes());
        for i in [0u32, 1, 2, 3, 0, 0, 0, 0] {
            b.extend(i.to_le_bytes());
        }
        b.extend(0u16.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        write_str(&mut b, "nose/nose-hump-incr");
        b.push(1);
        write_str(&mut b, "nose-hump-incr");
        b.extend((0.01f32 / 32767.0).to_le_bytes());
        b.extend(1u32.to_le_bytes());
        b.extend(2u32.to_le_bytes());
        for c in [0i16, 0, 32767] {
            b.extend(c.to_le_bytes());
        }
        b
    }

    #[test]
    fn parses_a_baked_pack() {
        let pack = HumanPack::parse(&tiny_pack_bytes()).unwrap();
        assert_eq!(pack.positions.len(), 4);
        assert_eq!(pack.quads[0].v, [0, 1, 2, 3]);
        let t = pack.target("nose/nose-hump-incr").unwrap();
        assert_eq!(t.factors, ["nose-hump-incr"]);
        assert_eq!(t.indices, [2]);
        assert!((t.deltas[0].z - 0.01).abs() < 1e-6);
        assert_eq!(pack.group_vertices("body"), [0, 1, 2, 3]);
    }

    #[test]
    fn damaged_packs_say_what_to_do() {
        let mut bytes = tiny_pack_bytes();
        bytes.truncate(40);
        let err = HumanPack::parse(&bytes).unwrap_err();
        assert!(err.contains("fetch-assets"), "{err}");
        assert!(HumanPack::parse(b"nope").is_err());
    }
}
