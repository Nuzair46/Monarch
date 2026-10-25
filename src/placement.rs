use crate::{Layout, ManagerError, OutputConfig, Position};

#[derive(Clone, Copy)]
struct Rect {
    x: i64,
    y: i64,
    width: i64,
    height: i64,
}

impl From<&OutputConfig> for Rect {
    fn from(output: &OutputConfig) -> Self {
        Self {
            x: output.position.x.into(),
            y: output.position.y.into(),
            width: output.resolution.width.into(),
            height: output.resolution.height.into(),
        }
    }
}

impl Rect {
    fn overlaps(self, other: Self) -> bool {
        self.x < other.x + other.width
            && other.x < self.x + self.width
            && self.y < other.y + other.height
            && other.y < self.y + self.height
    }

    fn edge_candidates(self, other: Self) -> Vec<Self> {
        let x = self
            .x
            .clamp(other.x - self.width + 1, other.x + other.width - 1);
        let y = self
            .y
            .clamp(other.y - self.height + 1, other.y + other.height - 1);
        let mut candidates = Vec::with_capacity(12);
        for y in [y, other.y, other.y + other.height - self.height] {
            for x in [other.x - self.width, other.x + other.width] {
                candidates.push(Self { x, y, ..self });
            }
        }
        for x in [x, other.x, other.x + other.width - self.width] {
            for y in [other.y - self.height, other.y + other.height] {
                candidates.push(Self { x, y, ..self });
            }
        }
        candidates
    }
}

/// Preserve connected groups, moving only groups separated by a missing monitor.
pub(crate) fn close_missing_display_gaps(layout: &mut Layout) -> Result<(), ManagerError> {
    layout.ensure_supported()?;
    let active: Vec<_> = layout
        .outputs
        .iter()
        .enumerate()
        .filter(|(_, o)| o.enabled)
        .map(|(i, _)| i)
        .collect();
    if active.len() < 2 {
        return Ok(());
    }
    if active
        .iter()
        .any(|&i| layout.outputs[i].resolution.width == 0)
    {
        return Err(ManagerError::Validation(
                "cannot close gaps after skipping a missing display with automatic monitor geometry; save the profile with explicit resolutions".into(),
            ));
    }
    let mut remaining = active;
    let mut groups = Vec::new();
    while let Some(first) = remaining.pop() {
        let mut group = vec![first];
        loop {
            let Some(index) = remaining.iter().position(|&i| {
                group.iter().any(|&j| {
                    layout.outputs[i].shares_source(&layout.outputs[j])
                        || Rect::from(&layout.outputs[i]).touches(Rect::from(&layout.outputs[j]))
                })
            }) else {
                break;
            };
            group.push(remaining.remove(index));
        }
        groups.push(group);
    }
    let primary_group = groups
        .iter()
        .position(|group| group.iter().any(|&i| layout.outputs[i].primary))
        .ok_or_else(|| ManagerError::Validation("gap closure requires a primary display".into()))?;
    let mut attached = groups.remove(primary_group);
    while !groups.is_empty() {
        let mut best = None;
        for (group_index, group) in groups.iter().enumerate() {
            for &i in group {
                let moving = Rect::from(&layout.outputs[i]);
                for &j in &attached {
                    let anchor = Rect::from(&layout.outputs[j]);
                    for candidate in moving.edge_candidates(anchor) {
                        let dx = candidate.x - moving.x;
                        let dy = candidate.y - moving.y;
                        let valid = group.iter().all(|&k| {
                            let old = Rect::from(&layout.outputs[k]);
                            let next = Rect {
                                x: old.x + dx,
                                y: old.y + dy,
                                ..old
                            };
                            next.x.abs() <= 1_000_000
                                && next.y.abs() <= 1_000_000
                                && layout
                                    .outputs
                                    .iter()
                                    .enumerate()
                                    .filter(|(n, o)| o.enabled && !group.contains(n))
                                    .all(|(_, o)| !next.overlaps(Rect::from(o)))
                        });
                        let distance = dx * dx + dy * dy;
                        if valid
                            && best
                                .as_ref()
                                .is_none_or(|&(score, _, _, _)| distance < score)
                        {
                            best = Some((distance, group_index, dx, dy));
                        }
                    }
                }
            }
        }
        let Some((_, index, dx, dy)) = best else {
            return Err(ManagerError::Validation(
                    "cannot close gaps left by missing displays without overlap; adjust and re-save the profile".into(),
                ));
        };
        let group = groups.remove(index);
        for &i in &group {
            layout.outputs[i].position.x = (i64::from(layout.outputs[i].position.x) + dx) as i32;
            layout.outputs[i].position.y = (i64::from(layout.outputs[i].position.y) + dy) as i32;
        }
        attached.extend(group);
    }
    layout.ensure_supported()
}

impl Rect {
    fn touches(self, other: Self) -> bool {
        ((self.x + self.width == other.x || other.x + other.width == self.x)
            && self.y < other.y + other.height
            && other.y < self.y + self.height)
            || ((self.y + self.height == other.y || other.y + other.height == self.y)
                && self.x < other.x + other.width
                && other.x < self.x + self.width)
    }
}

/// Keep a remembered position when it still fits. The desktop may have moved
/// while this monitor was detached, so otherwise join the nearest free edge.
pub(crate) fn place_attached_output(layout: &mut Layout, index: usize) -> Result<(), ManagerError> {
    layout.ensure_valid()?;
    let moving = Rect::from(&layout.outputs[index]);
    if moving.width == 0 {
        return Ok(()); // Windows must first resolve this target's automatic mode.
    }
    let others: Vec<Rect> = layout
        .outputs
        .iter()
        .enumerate()
        .filter(|(i, o)| *i != index && o.enabled && o.resolution.width > 0)
        .map(|(_, o)| Rect::from(o))
        .collect();
    let fits = |rect: Rect| {
        rect.x.abs() <= 1_000_000
            && rect.y.abs() <= 1_000_000
            && others.iter().any(|o| rect.touches(*o))
            && !others.iter().any(|o| rect.overlaps(*o))
    };
    if others.is_empty() || fits(moving) {
        return Ok(());
    }
    let mut candidates = Vec::new();
    for other in &others {
        candidates.extend(moving.edge_candidates(*other));
    }
    let position = candidates.into_iter().filter(|rect| fits(*rect))
        .min_by_key(|rect| (rect.x - moving.x).pow(2) + (rect.y - moving.y).pow(2))
        .ok_or_else(|| ManagerError::Validation("cannot place the attached monitor beside the active desktop; adjust the display arrangement and try again".into()))?;
    layout.outputs[index].position = Position {
        x: position.x as i32,
        y: position.y as i32,
    };
    Ok(())
}
