//! Typed geometry preferences. Entries are history, never evidence of a live route.
use crate::{identity, Layout};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GeometryHistory {
    version: u32,
    layout: Layout,
}

impl Default for GeometryHistory {
    fn default() -> Self {
        Self {
            version: 2,
            layout: Layout::default(),
        }
    }
}

impl GeometryHistory {
    pub fn is_valid(&self) -> bool {
        self.version == 2
            && self.layout.outputs.len() <= 128
            && self.layout.outputs.iter().all(|output| {
                // Different physical monitors can have used the same runtime address.
                // Validate each preference independently, not as a simultaneously active layout.
                let mut output = output.clone();
                output.enabled = true;
                Layout {
                    outputs: vec![output],
                }
                .ensure_valid()
                .is_ok()
            })
    }

    pub fn complete_inventory(&self, inventory: &mut Layout) {
        for output in &mut inventory.outputs {
            if output.enabled || output.resolution.width != 0 || output.resolution.height != 0 {
                continue;
            }
            if let identity::Resolution::Resolved(id) =
                identity::resolve(&output.display_id, &self.layout)
            {
                if let Some(previous) = self.layout.outputs.iter().find(|o| o.display_id == id) {
                    output.position = previous.position.clone();
                    output.resolution = previous.resolution.clone();
                    output.refresh_rate_mhz = previous.refresh_rate_mhz;
                    output.rotation = previous.rotation;
                }
            }
        }
    }

    pub fn remember(&mut self, observed: &Layout) {
        for output in &observed.outputs {
            let previous = match identity::resolve(&output.display_id, &self.layout) {
                identity::Resolution::Resolved(id) => Some(id),
                _ => None,
            };
            self.layout.outputs.retain(|o| {
                o.display_id != output.display_id && Some(&o.display_id) != previous.as_ref()
            });
            let mut remembered = output.clone();
            if let Some(previous) = previous {
                identity::preserve_evidence(&previous, &mut remembered.display_id);
            }
            self.layout.outputs.push(remembered);
        }
        let excess = self.layout.outputs.len().saturating_sub(128);
        self.layout.outputs.drain(..excess);
    }
}
