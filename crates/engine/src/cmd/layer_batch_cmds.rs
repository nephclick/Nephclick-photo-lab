//! Layer housekeeping over many layers at once, each one History step: Batch Rename (a base name with numbering,
//! find and replace, a prefix and a suffix) and Delete Empty Layers (pixel layers with nothing visible on the canvas,
//! then the groups left empty).

use std::collections::HashSet;
use std::sync::Arc;

use nephclick_photolab_color::SampleType;
use nephclick_photolab_doc::{Layer, LayerContent, LayerId};
use nephclick_photolab_geom::{Rect, TILE_SIZE, TileCoord};
use nephclick_photolab_raster::Surface;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

const RENAME: &str = "layer.batchRename";
const DELETE_EMPTY: &str = "layer.deleteEmpty";
/// The longest layer name Batch Rename writes (Photoshop's limit).
const MAX_NAME: usize = 255;
/// The most layers one rename takes.
const MAX_LAYERS: usize = 10_000;

fn bad(cmd: &str, message: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: message.into() }
}

fn has_layers(s: &Session) -> std::result::Result<(), String> {
    let st = s.active().ok_or("no document open")?;
    if st.selected_layers().is_empty() { Err("no layer selected".into()) } else { Ok(()) }
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

fn text(p: &Value, key: &str, cmd: &str) -> Result<String> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) if s.chars().count() <= MAX_NAME => Ok(s.clone()),
        Some(Value::String(_)) => Err(bad(cmd, format!("`{key}` is at most {MAX_NAME} characters"))),
        Some(other) => Err(bad(cmd, format!("`{key}` must be text, got {other}"))),
    }
}

fn number(p: &Value, key: &str, default: u64, max: u64, cmd: &str) -> Result<u64> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v.as_u64().filter(|n| *n <= max).ok_or_else(|| bad(cmd, format!("`{key}` must be a whole number 0..={max}"))),
    }
}

/// The settings of a Batch Rename.
pub struct RenamePlan {
    /// The base name; empty keeps each layer's own name.
    pub base: String,
    /// The first number.
    pub start: u64,
    /// The number's digits, padded with zeros (0: no padding).
    pub digits: usize,
    pub find: String,
    pub replace: String,
    pub prefix: String,
    pub suffix: String,
}

impl RenamePlan {
    fn of(p: &Value) -> Result<RenamePlan> {
        Ok(RenamePlan {
            base: text(p, "name", RENAME)?,
            start: number(p, "start", 1, 1_000_000, RENAME)?,
            digits: number(p, "digits", 0, 6, RENAME)? as usize,
            find: text(p, "find", RENAME)?,
            replace: text(p, "replace", RENAME)?,
            prefix: text(p, "prefix", RENAME)?,
            suffix: text(p, "suffix", RENAME)?,
        })
    }

    /// The new name of the layer named `old`, the `index`th renamed (0-based, top of the panel first).
    pub fn name(&self, old: &str, index: usize) -> String {
        let mut name = if self.base.is_empty() {
            old.to_owned()
        } else {
            let n = self.start.saturating_add(index as u64);
            format!("{} {:0width$}", self.base.trim_end(), n, width = self.digits)
        };
        if !self.find.is_empty() {
            name = name.replace(&self.find, &self.replace);
        }
        let name = format!("{}{}{}", self.prefix, name, self.suffix);
        name.chars().take(MAX_NAME).collect()
    }
}

/// The layers to rename, top of the Layers panel first: the given `layers` or the selected ones.
fn rename_targets(s: &Session, p: &Value) -> Result<Vec<LayerId>> {
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let ids: Vec<LayerId> = match p.get("layers") {
        None | Some(Value::Null) => st.selected_layers(),
        Some(Value::Array(a)) if a.len() <= MAX_LAYERS => {
            let mut ids = Vec::with_capacity(a.len());
            for v in a {
                let id = v.as_u64().map(LayerId).ok_or_else(|| bad(RENAME, format!("`layers` holds layer ids, got {v}")))?;
                if st.doc.layer(id).is_none() {
                    return Err(EngineError::NoLayer(id));
                }
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            ids
        }
        Some(v) => return Err(bad(RENAME, format!("`layers` is a list of at most {MAX_LAYERS} layer ids, got {v}"))),
    };
    if ids.is_empty() {
        return Err(bad(RENAME, "no layer selected"));
    }
    // The document lists layers bottom first; the panel, and the numbering, go top first.
    let order: Vec<LayerId> = st.doc.walk().iter().rev().map(|(_, _, l)| l.id).collect();
    let mut ids = ids;
    ids.sort_by_key(|id| order.iter().position(|o| o == id).unwrap_or(usize::MAX));
    Ok(ids)
}

fn batch_rename(s: &mut Session, p: &Value) -> Result<Value> {
    p.as_object().ok_or_else(|| bad(RENAME, "expected an object"))?;
    let plan = RenamePlan::of(p)?;
    if plan.base.is_empty() && plan.find.is_empty() && plan.prefix.is_empty() && plan.suffix.is_empty() {
        return Err(bad(RENAME, "give a `name`, a `find`, a `prefix` or a `suffix`"));
    }
    let ids = rename_targets(s, p)?;
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let renames: Vec<(LayerId, String)> = ids
        .iter()
        .enumerate()
        .filter_map(|(i, id)| st.doc.layer(*id).map(|l| (*id, plan.name(&l.name, i))))
        .filter(|(id, name)| !name.trim().is_empty() && st.doc.layer(*id).is_some_and(|l| l.name != *name))
        .collect();
    if !renames.is_empty() {
        s.edit("Batch Rename", |doc, _| {
            for (id, name) in &renames {
                doc.layer_mut(*id).ok_or(EngineError::NoLayer(*id))?.name.clone_from(name);
            }
            Ok(())
        })?;
        if let Some(st) = s.active_mut() {
            // Names only: nothing on the canvas changes.
            st.last_damage = Some(Rect::EMPTY);
        }
    }
    Ok(json!({"renamed": renames.iter().map(|(id, name)| json!({"layer": id.0, "name": name})).collect::<Vec<_>>()}))
}

// ---------------------------------------------------------------------------------------------
// Empty layers

/// Whether `l` is an empty pixel layer: no pixel with any opacity on the canvas `canvas` (the whole layer without
/// one). Other kinds (text, shapes, adjustments, fills, smart objects, groups) are never empty here.
pub fn is_empty_layer(l: &Layer, canvas: Option<Rect>) -> bool {
    matches!(&l.content, LayerContent::Raster(s) if !has_visible_pixels(s, canvas))
}

fn has_visible_pixels(s: &Surface, canvas: Option<Rect>) -> bool {
    let fmt = s.format();
    if !fmt.alpha {
        return true;
    }
    let clip = match (s.default_pixel().last().is_none_or(|a| *a > 0.0), canvas) {
        (false, _) => None,
        (true, Some(canvas)) if covers(s, canvas) => Some(canvas),
        (true, _) => return true,
    };
    let (n, bpp, size) = (fmt.channels(), fmt.bytes_per_pixel(), fmt.sample.bytes());
    let Some(at) = n.checked_sub(1).and_then(|i| i.checked_mul(size)).filter(|_| bpp > 0) else { return false };
    // Integer alpha is above 0 where any of its bytes is, so a block of pixels is OR-ed together (which vectorises)
    // before looking for one; only float alpha needs decoding.
    let visible = |block: &[u8]| match fmt.sample {
        SampleType::F32 => block.chunks_exact(bpp).any(|px| px.get(at..at + size).is_some_and(|a| a.try_into().is_ok_and(|b| f32::from_ne_bytes(b) > 0.0))),
        SampleType::U8 | SampleType::U16 => {
            block.chunks_exact(bpp).fold(0u8, |acc, px| px.get(at..at + size).map_or(acc, |a| a.iter().fold(acc, |acc, b| acc | b))) != 0
        }
    };
    let tile_visible = |c: &TileCoord, t: &Arc<nephclick_photolab_raster::Tile>| {
        let bytes = t.bytes();
        let r = c.rect();
        let Some(clip) = clip.filter(|clip| !clip.contains_rect(&r)) else { return bytes.chunks(bpp.saturating_mul(1024)).any(visible) };
        // Only the part of the tile on the canvas, row by row.
        let on = r.intersect(&clip);
        let row = (TILE_SIZE as usize).saturating_mul(bpp);
        let (x0, x1) = ((on.x0 - r.x0) as usize * bpp, (on.x1 - r.x0) as usize * bpp);
        (on.y0 - r.y0..on.y1 - r.y0).any(|y| bytes.get(y as usize * row + x0..y as usize * row + x1).is_some_and(visible))
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        let tiles: Vec<_> = s.tiles().filter(|(c, _)| clip.is_none_or(|clip| !c.rect().intersect(&clip).is_empty())).collect();
        tiles.par_iter().any(|(c, t)| tile_visible(c, t))
    }
    #[cfg(target_arch = "wasm32")]
    s.tiles().filter(|(c, _)| clip.is_none_or(|clip| !c.rect().intersect(&clip).is_empty())).any(|(c, t)| tile_visible(c, t))
}

fn covers(s: &Surface, canvas: Rect) -> bool {
    if canvas.is_empty() {
        return true;
    }
    let (first, last) = (TileCoord::containing(canvas.x0, canvas.y0), TileCoord::containing(canvas.x1 - 1, canvas.y1 - 1));
    let needed = (i64::from(last.tx) - i64::from(first.tx) + 1) * (i64::from(last.ty) - i64::from(first.ty) + 1);
    let stored = s.tiles().filter(|(c, _)| (first.tx..=last.tx).contains(&c.tx) && (first.ty..=last.ty).contains(&c.ty)).count();
    i64::try_from(stored).is_ok_and(|stored| stored == needed)
}

/// The layers Delete Empty Layers removes: the empty pixel layers (not the Background), then each group whose
/// layers all go (a group with nothing in it as well).
pub fn empty_layers(doc: &nephclick_photolab_doc::Document) -> Vec<LayerId> {
    let canvas = Some(doc.bounds());
    let mut gone: HashSet<LayerId> =
        doc.walk().iter().filter(|(_, _, l)| !doc.is_background(l.id) && is_empty_layer(l, canvas)).map(|(_, _, l)| l.id).collect();
    // Groups empty once their contents go, innermost first: repeat until nothing more goes (bounded by the depth).
    for _ in 0..64 {
        let more: Vec<LayerId> = doc
            .walk()
            .iter()
            .filter(|(_, _, l)| !gone.contains(&l.id))
            .filter_map(|(_, _, l)| l.children().filter(|ch| ch.iter().all(|c| gone.contains(&c.id))).map(|_| l.id))
            .collect();
        if more.is_empty() {
            break;
        }
        gone.extend(more);
    }
    // Keep the outermost only: removing a group removes what it holds.
    let walk = doc.walk();
    walk.iter()
        .filter(|(path, _, l)| {
            gone.contains(&l.id)
                && !walk.iter().any(|(other, _, o)| gone.contains(&o.id) && other.len() < path.len() && path.starts_with(other))
        })
        .map(|(_, _, l)| l.id)
        .collect()
}

fn delete_empty(s: &mut Session, _p: &Value) -> Result<Value> {
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let mut ids = empty_layers(&st.doc);
    // A document keeps at least one layer: when every layer is empty, the top one stays.
    let top: Vec<LayerId> = st.doc.layers.iter().map(|l| l.id).collect();
    if top.iter().all(|t| ids.contains(t))
        && let Some(keep) = top.last()
    {
        ids.retain(|id| id != keep);
    }
    if ids.is_empty() {
        return Ok(json!({"deleted": []}));
    }
    s.edit("Delete Empty Layers", |doc, active| {
        let before = crate::clip_group::Before::read(doc, &ids);
        for id in &ids {
            doc.remove(*id);
        }
        crate::clip_group::settle(doc, &[], &before, false);
        if doc.layers.is_empty() {
            return Err(EngineError::Other("every layer is empty: a document must keep at least one layer".into()));
        }
        if active.is_some_and(|current| doc.layer(current).is_none()) {
            *active = doc.walk().last().map(|(_, _, l)| l.id);
        }
        Ok(())
    })?;
    Ok(json!({"deleted": ids.iter().map(|id| id.0).collect::<Vec<_>>()}))
}

/// Batch Rename and Delete Empty Layers.
pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: RENAME,
            label: "Batch Rename Layers",
            menu: &["Layer"],
            shortcut: None,
            params: r#"{"name":str?=keep (base name, numbered),"start":uint=1,"digits":0..6=0,"find":str?,"replace":str="","prefix":str="","suffix":str="","layers":[id,…]? (default: the selected layers; numbered from the top of the Layers panel)}"#,
            enabled: has_layers,
            run: batch_rename,
            journal: true,
        },
        CommandSpec {
            id: DELETE_EMPTY,
            label: "Delete Empty Layers",
            menu: &["Layer", "Delete"],
            shortcut: None,
            params: r#"{} (pixel layers with nothing on the canvas, then groups left empty; never the Background)"#,
            enabled: has_doc,
            run: delete_empty,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 32, "height": 32, "background": "transparent"})).unwrap();
        s
    }

    #[test]
    fn rename_numbers_from_the_top_with_padding_and_find_replace() {
        let plan = RenamePlan {
            base: "Shot".into(),
            start: 1,
            digits: 2,
            find: "Shot".into(),
            replace: "Take".into(),
            prefix: "A-".into(),
            suffix: "!".into(),
        };
        assert_eq!(plan.name("old", 0), "A-Take 01!");
        assert_eq!(plan.name("old", 9), "A-Take 10!");
        let keep = RenamePlan { base: String::new(), start: 1, digits: 0, find: "cat".into(), replace: "dog".into(), prefix: String::new(), suffix: String::new() };
        assert_eq!(keep.name("my cat", 3), "my dog");
    }

    #[test]
    fn batch_rename_renames_the_selected_layers_in_one_step() {
        let mut s = session();
        s.execute("layer.new.layer", json!({})).unwrap();
        s.execute("layer.new.layer", json!({})).unwrap();
        let ids: Vec<u64> = s.active().unwrap().doc.walk().iter().map(|(_, _, l)| l.id.0).collect();
        s.execute("layer.select", json!({"layers": ids})).unwrap();
        let steps = s.journal.len();
        let r = s.execute(RENAME, json!({"name": "Part", "digits": 2})).unwrap();
        assert_eq!(r["renamed"].as_array().map(Vec::len), Some(ids.len()));
        let names: Vec<String> = s.active().unwrap().doc.walk().iter().rev().map(|(_, _, l)| l.name.clone()).collect();
        assert_eq!(names, (1..=ids.len()).map(|n| format!("Part {n:02}")).collect::<Vec<_>>(), "top of the panel is 01");
        assert_eq!(s.journal.len(), steps + 1, "one History step");
    }

    #[test]
    fn batch_rename_rejects_bad_params() {
        let mut s = session();
        for p in [json!({}), json!({"name": 3}), json!({"name": "a", "digits": 9}), json!({"name": "a", "layers": [999999]}), json!("x")] {
            assert!(s.execute(RENAME, p.clone()).is_err(), "{p}");
        }
    }

    #[test]
    fn delete_empty_layers_removes_empty_pixel_layers_and_emptied_groups() {
        let mut s = session();
        s.execute("layer.new.layer", json!({})).unwrap();
        s.execute("layer.new.layer", json!({})).unwrap();
        s.execute("edit.fill", json!({"color": "#ff0000"})).unwrap();
        let before = s.active().unwrap().doc.walk().len();
        let r = s.execute(DELETE_EMPTY, json!({})).unwrap();
        let deleted = r["deleted"].as_array().map(Vec::len).unwrap_or(0);
        assert!(deleted >= 1, "{r}");
        let after = s.active().unwrap().doc.walk().len();
        assert_eq!(after, before - deleted);
        assert!(s.active().unwrap().doc.walk().iter().all(|(_, _, l)| !is_empty_layer(l, None) || s.active().unwrap().doc.is_background(l.id)));
        // Nothing left to delete: no step.
        assert_eq!(s.execute(DELETE_EMPTY, json!({})).unwrap()["deleted"], json!([]));
    }
}
