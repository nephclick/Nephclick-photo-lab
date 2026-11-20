//! The colour look-up tables RAW Develop applies (`RawDevelopSettings::lut`): the built-in looks and the LUTs the user
//! loaded into the library. The develop finds a LUT by its name; a name the library does not hold leaves the picture
//! as it is, so a document whose LUT is missing on this machine still opens and develops.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

/// Most LUTs the library holds besides the built-in looks.
pub const MAX_LUTS: usize = 500;

/// Longest LUT name the library takes, in bytes (the settings' own limit).
pub const MAX_NAME: usize = craft::imaging::raw::MAX_LUT_NAME;

/// Largest edge length of a LUT the library takes (the `.cube` reader's limit).
pub const MAX_SIZE: usize = 129;

/// A 3D colour LUT: `size`³ RGB triplets in 0..=1, red varying fastest.
#[derive(Clone, Debug, PartialEq)]
pub struct RawLut {
    size: usize,
    data: Vec<f32>,
}

impl RawLut {
    /// The LUT of edge `size` over `data`; `None` for a size outside 2..=[`MAX_SIZE`], a table too short for it, or a
    /// value that is not finite.
    pub fn new(size: usize, mut data: Vec<f32>) -> Option<Self> {
        if !(2..=MAX_SIZE).contains(&size) {
            return None;
        }
        let len = size.checked_pow(3)?.checked_mul(3)?;
        if data.len() < len {
            return None;
        }
        data.truncate(len);
        data.iter().all(|v| v.is_finite()).then_some(RawLut { size, data })
    }

    /// The edge length.
    pub fn size(&self) -> usize {
        self.size
    }

    /// The colour `c` (0..=1, clamped) looked up with trilinear interpolation.
    pub fn sample(&self, c: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let last = (n - 1) as f32;
        let mut base = [0usize; 3];
        let mut frac = [0.0f32; 3];
        for k in 0..3 {
            let v = if c[k].is_finite() { c[k].clamp(0.0, 1.0) } else { 0.0 } * last;
            let i = (v.floor() as usize).min(n - 2);
            base[k] = i;
            frac[k] = v - i as f32;
        }
        let at = |r: usize, g: usize, b: usize, ch: usize| -> f32 {
            let i = ((b * n + g) * n + r) * 3 + ch;
            self.data.get(i).copied().unwrap_or(0.0)
        };
        let [r0, g0, b0] = base;
        let [fr, fg, fb] = frac;
        std::array::from_fn(|ch| {
            let c00 = at(r0, g0, b0, ch) * (1.0 - fr) + at(r0 + 1, g0, b0, ch) * fr;
            let c10 = at(r0, g0 + 1, b0, ch) * (1.0 - fr) + at(r0 + 1, g0 + 1, b0, ch) * fr;
            let c01 = at(r0, g0, b0 + 1, ch) * (1.0 - fr) + at(r0 + 1, g0, b0 + 1, ch) * fr;
            let c11 = at(r0, g0 + 1, b0 + 1, ch) * (1.0 - fr) + at(r0 + 1, g0 + 1, b0 + 1, ch) * fr;
            let c0 = c00 * (1.0 - fg) + c10 * fg;
            let c1 = c01 * (1.0 - fg) + c11 * fg;
            c0 * (1.0 - fb) + c1 * fb
        })
    }
}

/// The library: the built-in looks, then what the user loaded.
fn library() -> &'static RwLock<BTreeMap<String, Arc<RawLut>>> {
    static LIBRARY: OnceLock<RwLock<BTreeMap<String, Arc<RawLut>>>> = OnceLock::new();
    LIBRARY.get_or_init(|| RwLock::new(builtins()))
}

/// The built-in looks, by their names.
fn builtins() -> BTreeMap<String, Arc<RawLut>> {
    craft_cms::lutfile::BUILTIN
        .iter()
        .filter_map(|(id, label)| {
            let file = craft_cms::lutfile::builtin(id)?;
            Some(((*label).to_owned(), Arc::new(RawLut::new(file.size, file.data)?)))
        })
        .collect()
}

/// The names of the built-in looks, in their order.
pub fn builtin_names() -> Vec<String> {
    craft_cms::lutfile::BUILTIN.iter().map(|(_, label)| (*label).to_owned()).collect()
}

/// Whether `name` is a built-in look.
pub fn is_builtin(name: &str) -> bool {
    craft_cms::lutfile::BUILTIN.iter().any(|(_, label)| *label == name)
}

/// Every LUT's name: the built-in looks in their order, then the user's by name.
pub fn names() -> Vec<String> {
    let mut out = builtin_names();
    let held = library().read().unwrap_or_else(PoisonError::into_inner);
    out.extend(held.keys().filter(|name| !is_builtin(name)).cloned());
    out
}

/// The LUT named `name`.
pub fn get(name: &str) -> Option<Arc<RawLut>> {
    library().read().unwrap_or_else(PoisonError::into_inner).get(name).cloned()
}

/// Adds (or replaces) the user's LUT `name`. Refuses an empty or over-long name, a built-in look's name, and a library
/// already holding [`MAX_LUTS`] of the user's.
pub fn insert(name: &str, lut: RawLut) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_NAME {
        return Err(format!("a LUT name is 1 to {MAX_NAME} bytes"));
    }
    if is_builtin(name) {
        return Err(format!("\"{name}\" is a built-in look"));
    }
    let mut held = library().write().unwrap_or_else(PoisonError::into_inner);
    let user = held.keys().filter(|key| !is_builtin(key)).count();
    if !held.contains_key(name) && user >= MAX_LUTS {
        return Err(format!("the LUT library holds at most {MAX_LUTS} LUTs"));
    }
    held.insert(name.to_owned(), Arc::new(lut));
    Ok(())
}

/// Removes the user's LUT `name`; built-in looks stay.
pub fn remove(name: &str) {
    if !is_builtin(name) {
        library().write().unwrap_or_else(PoisonError::into_inner).remove(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_lut_samples_the_colour_it_is_given() {
        let n = 3;
        let mut data = Vec::new();
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    data.extend([r as f32 / 2.0, g as f32 / 2.0, b as f32 / 2.0]);
                }
            }
        }
        let lut = RawLut::new(n, data).unwrap();
        let out = lut.sample([0.3, 0.6, 0.9]);
        for (a, b) in out.iter().zip([0.3, 0.6, 0.9]) {
            assert!((a - b).abs() < 1e-5);
        }
        // Hostile input is clamped, never indexed out of the table.
        let out = lut.sample([f32::NAN, 7.0, -3.0]);
        assert!(out.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn bad_tables_are_refused() {
        assert!(RawLut::new(1, vec![0.0; 3]).is_none());
        assert!(RawLut::new(2, vec![0.0; 23]).is_none());
        assert!(RawLut::new(2, vec![f32::NAN; 24]).is_none());
        assert!(RawLut::new(MAX_SIZE + 1, Vec::new()).is_none());
    }

    #[test]
    fn built_in_looks_are_in_the_library() {
        let names = builtin_names();
        assert!(!names.is_empty());
        assert!(names.iter().all(|name| get(name).is_some()));
        assert!(insert(&names[0], RawLut::new(2, vec![0.0; 24]).unwrap()).is_err());
    }
}
