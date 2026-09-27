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
        let x = moving
            .x
            .clamp(other.x - moving.width + 1, other.x + other.width - 1);
        let y = moving
            .y
            .clamp(other.y - moving.height + 1, other.y + other.height - 1);
        for y in [y, other.y, other.y + other.height - moving.height] {
            for x in [other.x - moving.width, other.x + other.width] {
                candidates.push(Rect { x, y, ..moving });
            }
        }
        for x in [x, other.x, other.x + other.width - moving.width] {
            for y in [other.y - moving.height, other.y + other.height] {
                candidates.push(Rect { x, y, ..moving });
            }
        }
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
