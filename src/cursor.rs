//! Boundary-only cursor mapping. All units are desktop pixels or physical mm;
//! Windows DPI scale factors never change pointer speed inside a surface.
use crate::{identity, DisplayId, Layout, ManagerError, MonitorIdentity, Position, Rotation};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    pub display_key: String,
    #[serde(default)]
    pub identity: MonitorIdentity,
    /// Unrotated panel dimensions (EDID or manual measurements).
    pub width_mm: u32,
    pub height_mm: u32,
    /// Top-left of the physically arranged, rotated monitor, in mm.
    pub position_mm: Position,
    #[serde(default)]
    pub clone_representative: bool,
}
impl Calibration {
    pub fn display_id(&self) -> Result<DisplayId, ManagerError> {
        let mut id = identity::parse_display_key(&self.display_key)?;
        id.identity = self.identity.clone();
        Ok(id)
    }
    pub fn is_valid(&self) -> bool {
        (10..=10000).contains(&self.width_mm)
            && (10..=10000).contains(&self.height_mm)
            && self.position_mm.x.unsigned_abs() <= 1_000_000
            && self.position_mm.y.unsigned_abs() <= 1_000_000
            && self.display_id().is_ok_and(|id| {
                id.edid_hash.is_some()
                    || id.identity.device_path.is_some()
                    || id.identity.edid_serial.is_some()
            })
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
impl Rect {
    fn contains(self, p: Point) -> bool {
        p.x >= self.left && p.x < self.right && p.y >= self.top && p.y < self.bottom
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    pub display_id: DisplayId,
    pub pixels: Rect,
    physical: [f64; 4],
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}
#[derive(Clone, Debug, PartialEq)]
struct Boundary {
    from: usize,
    to: usize,
    edge: Edge,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mapping {
    pub surfaces: Vec<Surface>,
    pub desktop: Rect,
    boundaries: Vec<Boundary>,
    pub issues: Vec<CalibrationIssue>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CalibrationIssue {
    pub display_key: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct CursorStatus {
    pub platform_supported: bool,
    pub enabled: bool,
    pub running: bool,
    pub calibrated_monitors: usize,
    pub boundaries: usize,
    pub corrected_crossings: u64,
    pub input_events: u64,
    pub pause_reason: Option<String>,
    pub issues: Vec<CalibrationIssue>,
}
#[derive(Clone, Copy)]
pub struct InputContext {
    pub injected: bool,
    pub control_down: bool,
    pub confined: bool,
    pub input_desktop_available: bool,
}
impl InputContext {
    pub fn allows_correction(self) -> bool {
        !self.injected && !self.control_down && !self.confined && self.input_desktop_available
    }
}

/// The same event sequence state used by the native hook. A corrected position
/// becomes the next origin only after Windows accepts the cursor movement.
#[derive(Default)]
pub struct MotionTracker {
    previous: Option<Point>,
}
impl MotionTracker {
    pub fn reset(&mut self) {
        self.previous = None;
    }
    pub fn movement(
        &mut self,
        mapping: &Mapping,
        proposed: Point,
        context: InputContext,
    ) -> Option<Point> {
        if !context.allows_correction() {
            self.reset();
            return None;
        }
        self.previous
            .replace(proposed)
            .and_then(|previous| mapping.map_motion(previous, proposed, context))
    }
    pub fn accepted(&mut self, corrected: Point) {
        self.previous = Some(corrected);
    }
}
impl Mapping {
    pub fn build(layout: &Layout, calibrations: &[Calibration]) -> Self {
        if let Err(error) = layout.ensure_supported() {
            return Self {
                issues: vec![CalibrationIssue {
                    display_key: None,
                    message: error.to_string(),
                }],
                ..Self::default()
            };
        }
        let mut outputs: Vec<_> = layout
            .outputs
            .iter()
            .filter(|o| o.enabled && o.resolution.width > 0)
            .collect();
        outputs.sort_by(|a, b| {
            a.display_id
                .identity
                .edid_serial
                .cmp(&b.display_id.identity.edid_serial)
                .then(
                    a.display_id
                        .identity
                        .device_path
                        .cmp(&b.display_id.identity.device_path),
                )
                .then(a.display_id.edid_hash.cmp(&b.display_id.edid_hash))
                .then(a.display_id.endpoint().cmp(&b.display_id.endpoint()))
        });
        let resolved: Vec<_> = calibrations
            .iter()
            .filter(|c| c.is_valid())
            .filter_map(|c| match identity::resolve(&c.display_id().ok()?, layout) {
                identity::Resolution::Resolved(id) => Some((id, c)),
                _ => None,
            })
            .collect();
        let calibration = |id: &DisplayId| {
            let matches: Vec<_> = resolved.iter().filter(|(saved, _)| saved == id).collect();
            (matches.len() == 1).then(|| matches[0].1)
        };
        let mut result = Self::default();
        if outputs.is_empty() {
            return result;
        }
        result.desktop = Rect {
            left: outputs.iter().map(|o| o.position.x).min().unwrap(),
            top: outputs.iter().map(|o| o.position.y).min().unwrap(),
            right: outputs
                .iter()
                .map(|o| o.position.x + o.resolution.width as i32)
                .max()
                .unwrap(),
            bottom: outputs
                .iter()
                .map(|o| o.position.y + o.resolution.height as i32)
                .max()
                .unwrap(),
        };
        let mut seen = std::collections::HashSet::new();
        for output in &outputs {
            if !seen.insert(output.display_id.endpoint()) {
                continue;
            }
            let members: Vec<_> = outputs
                .iter()
                .copied()
                .filter(|o| o.display_id == output.display_id || o.shares_source(output))
                .collect();
            for member in &members {
                seen.insert(member.display_id.endpoint());
            }
            let representative = members
                .iter()
                .copied()
                .find(|o| calibration(&o.display_id).is_some_and(|c| c.clone_representative))
                .unwrap_or(members[0]);
            let Some(cal) = calibration(&representative.display_id) else {
                result.issues.push(CalibrationIssue {
                    display_key: Some(identity::display_key(&representative.display_id)),
                    message:
                        "Enter valid physical dimensions and enable this monitor’s calibration."
                            .into(),
                });
                continue;
            };
            let (w, h) = if matches!(
                representative.rotation,
                Some(Rotation::Portrait | Rotation::PortraitFlipped)
            ) {
                (cal.height_mm, cal.width_mm)
            } else {
                (cal.width_mm, cal.height_mm)
            };
            result.surfaces.push(Surface {
                display_id: representative.display_id.clone(),
                pixels: Rect {
                    left: output.position.x,
                    top: output.position.y,
                    right: output.position.x + output.resolution.width as i32,
                    bottom: output.position.y + output.resolution.height as i32,
                },
                physical: [
                    f64::from(cal.position_mm.x),
                    f64::from(cal.position_mm.y),
                    f64::from(w),
                    f64::from(h),
                ],
            });
        }
        // Overlapping physical calibrations are ambiguous. Exclude both, never guess.
        let overlaps: Vec<_> = result
            .surfaces
            .iter()
            .enumerate()
            .filter_map(|(i, a)| {
                result
                    .surfaces
                    .iter()
                    .enumerate()
                    .any(|(j, b)| {
                        i != j
                            && a.physical[0] < b.physical[0] + b.physical[2] - 0.5
                            && b.physical[0] < a.physical[0] + a.physical[2] - 0.5
                            && a.physical[1] < b.physical[1] + b.physical[3] - 0.5
                            && b.physical[1] < a.physical[1] + a.physical[3] - 0.5
                    })
                    .then_some(i)
            })
            .collect();
        for i in &overlaps {
            result.issues.push(CalibrationIssue {
                display_key: Some(identity::display_key(&result.surfaces[*i].display_id)),
                message:
                    "Physical monitor rectangles overlap. Drag them apart so their edges meet."
                        .into(),
            });
        }
        result.surfaces = result
            .surfaces
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !overlaps.contains(i))
            .map(|(_, s)| s)
            .collect();
        for (from, a) in result.surfaces.iter().enumerate() {
            for (to, b) in result.surfaces.iter().enumerate() {
                if from == to {
                    continue;
                }
                let [ax, ay, aw, ah] = a.physical;
                let [bx, by, bw, bh] = b.physical;
                let vertical = ay < by + bh && by < ay + ah;
                let horizontal = ax < bx + bw && bx < ax + aw;
                let edge = if vertical && (ax + aw - bx).abs() <= 1.0 {
                    Some(Edge::Right)
                } else if vertical && (bx + bw - ax).abs() <= 1.0 {
                    Some(Edge::Left)
                } else if horizontal && (ay + ah - by).abs() <= 1.0 {
                    Some(Edge::Bottom)
                } else if horizontal && (by + bh - ay).abs() <= 1.0 {
                    Some(Edge::Top)
                } else {
                    None
                };
                if let Some(edge) = edge {
                    result.boundaries.push(Boundary { from, to, edge });
                }
            }
        }
        for (index, surface) in result.surfaces.iter().enumerate() {
            if !result.boundaries.iter().any(|b| b.from == index) {
                result.issues.push(CalibrationIssue {
                    display_key: Some(identity::display_key(&surface.display_id)),
                    message: "No adjoining calibrated monitor. Drag the physical edges together."
                        .into(),
                });
            }
        }
        result
    }

    pub fn boundary_count(&self) -> usize {
        self.boundaries.len() / 2
    }

    pub fn status(&self, enabled: bool) -> CursorStatus {
        CursorStatus {
            enabled,
            calibrated_monitors: self.surfaces.len(),
            boundaries: self.boundary_count(),
            issues: self.issues.clone(),
            ..CursorStatus::default()
        }
    }
    pub fn map_motion(
        &self,
        previous: Point,
        proposed: Point,
        context: InputContext,
    ) -> Option<Point> {
        if !context.allows_correction() {
            return None;
        }
        let mut source = self
            .surfaces
            .iter()
            .position(|s| s.pixels.contains(previous))?;
        let mut start = (f64::from(previous.x), f64::from(previous.y));
        let mut end = (f64::from(proposed.x), f64::from(proposed.y));
        let mut corrected = false;
        for _ in 0..self.surfaces.len() {
            let rect = self.surfaces[source].pixels;
            let dx = end.0 - start.0;
            let dy = end.1 - start.1;
            // Include the last edge pixel to handle native dead-end clipping.
            // Stack-only candidates keep the low-level callback allocation-free.
            let exits = [
                (dx > 0.0 && end.0 >= f64::from(rect.right - 1))
                    .then(|| ((f64::from(rect.right - 1) - start.0) / dx, Edge::Right)),
                (dx < 0.0 && end.0 <= f64::from(rect.left))
                    .then(|| ((f64::from(rect.left) - start.0) / dx, Edge::Left)),
                (dy > 0.0 && end.1 >= f64::from(rect.bottom - 1))
                    .then(|| ((f64::from(rect.bottom - 1) - start.1) / dy, Edge::Bottom)),
                (dy < 0.0 && end.1 <= f64::from(rect.top))
                    .then(|| ((f64::from(rect.top) - start.1) / dy, Edge::Top)),
            ];
            let Some((t, edge)) = exits
                .into_iter()
                .flatten()
                .filter(|(t, _)| *t >= 0.0 && *t <= 1.0)
                .min_by(|a, b| a.0.total_cmp(&b.0))
            else {
                let point = Point {
                    x: end.0.round() as i32,
                    y: end.1.round() as i32,
                };
                return (corrected && rect.contains(point)).then_some(point);
            };
            let cross = (start.0 + dx * t, start.1 + dy * t);
            let from = &self.surfaces[source];
            let physical_y = from.physical[1]
                + (cross.1 - f64::from(rect.top)) / f64::from(rect.bottom - rect.top)
                    * from.physical[3];
            let physical_x = from.physical[0]
                + (cross.0 - f64::from(rect.left)) / f64::from(rect.right - rect.left)
                    * from.physical[2];
            let mut candidates = self
                .boundaries
                .iter()
                .filter(|b| b.from == source && b.edge == edge)
                .filter(|b| {
                    let p = self.surfaces[b.to].physical;
                    match edge {
                        Edge::Left | Edge::Right => physical_y >= p[1] && physical_y < p[1] + p[3],
                        _ => physical_x >= p[0] && physical_x < p[0] + p[2],
                    }
                });
            let Some(candidate) = candidates.next() else {
                // A fast move may cross a valid boundary and then run beyond the
                // desktop. Keep the corrected physical height at the final edge.
                return corrected.then(|| Point {
                    x: end
                        .0
                        .round()
                        .clamp(f64::from(rect.left), f64::from(rect.right - 1))
                        as i32,
                    y: end
                        .1
                        .round()
                        .clamp(f64::from(rect.top), f64::from(rect.bottom - 1))
                        as i32,
                });
            };
            if candidates.next().is_some() {
                return None;
            }
            source = candidate.to;
            let to = &self.surfaces[source];
            let r = to.pixels;
            let p = to.physical;
            let mapped_x = (f64::from(r.left)
                + (physical_x - p[0]) / p[2] * f64::from(r.right - r.left))
            .clamp(f64::from(r.left), f64::from(r.right - 1));
            let mapped_y = (f64::from(r.top)
                + (physical_y - p[1]) / p[3] * f64::from(r.bottom - r.top))
            .clamp(f64::from(r.top), f64::from(r.bottom - 1));
            start = match edge {
                Edge::Right => (f64::from(r.left + 1), mapped_y),
                Edge::Left => (f64::from(r.right - 2), mapped_y),
                Edge::Bottom => (mapped_x, f64::from(r.top + 1)),
                Edge::Top => (mapped_x, f64::from(r.bottom - 2)),
            };
            // Retain the remaining native displacement after crossing. Pointer
            // acceleration and speed within each monitor remain Windows' job.
            end = (start.0 + dx * (1.0 - t), start.1 + dy * (1.0 - t));
            corrected = true;
            if r.contains(Point {
                x: end.0.round() as i32,
                y: end.1.round() as i32,
            }) {
                return Some(Point {
                    x: end.0.round() as i32,
                    y: end.1.round() as i32,
                });
            }
        }
        None
    }
}
