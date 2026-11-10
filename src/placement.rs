// SPDX-License-Identifier: GPL-3.0

//! Which screen corner the sidebar sits in. Dragging it saves the choice to its own state
//! file, so the settings window rewriting `config.toml` can't overwrite it.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    pub fn is_left(self) -> bool {
        matches!(self, Corner::TopLeft | Corner::BottomLeft)
    }

    pub fn is_top(self) -> bool {
        matches!(self, Corner::TopLeft | Corner::TopRight)
    }

    /// The corner of a `width` x `height` area nearest to a point inside it.
    pub fn nearest(x: f32, y: f32, width: f32, height: f32) -> Corner {
        match (x < width / 2.0, y < height / 2.0) {
            (true, true) => Corner::TopLeft,
            (false, true) => Corner::TopRight,
            (true, false) => Corner::BottomLeft,
            (false, false) => Corner::BottomRight,
        }
    }

    /// The top left of a `size` box placed in this corner of `area`, inset by `margin`.
    pub fn origin(self, area: (f32, f32), size: (f32, f32), margin: f32) -> (f32, f32) {
        let x = if self.is_left() {
            margin
        } else {
            area.0 - margin - size.0
        };
        let y = if self.is_top() {
            margin
        } else {
            area.1 - margin - size.1
        };
        (x, y)
    }
}

/// The saved spot: a corner, and the output (connector name, e.g. `DP-1`) it was dropped on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    #[serde(default)]
    pub corner: Corner,
    /// `None` opens on the focused output.
    #[serde(default)]
    pub output: Option<String>,
}

/// `$XDG_STATE_HOME/cosmic-deploy-sidebar/placement.toml`.
fn path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("state"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("cosmic-deploy-sidebar").join("placement.toml")
}

/// The saved placement, or the top right of the focused output.
pub fn load() -> Placement {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

/// Remember where the sidebar was dropped.
pub fn save(placement: &Placement) -> Result<(), String> {
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let text = toml::to_string(placement).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_picks_the_quadrant() {
        assert_eq!(Corner::nearest(10.0, 10.0, 100.0, 50.0), Corner::TopLeft);
        assert_eq!(Corner::nearest(90.0, 10.0, 100.0, 50.0), Corner::TopRight);
        assert_eq!(Corner::nearest(10.0, 40.0, 100.0, 50.0), Corner::BottomLeft);
        assert_eq!(
            Corner::nearest(90.0, 40.0, 100.0, 50.0),
            Corner::BottomRight
        );
    }

    #[test]
    fn origin_insets_from_the_corner() {
        let area = (1000.0, 800.0);
        let size = (300.0, 200.0);
        assert_eq!(Corner::TopLeft.origin(area, size, 12.0), (12.0, 12.0));
        assert_eq!(Corner::BottomRight.origin(area, size, 12.0), (688.0, 588.0));
    }

    #[test]
    fn placement_round_trips() {
        let placement = Placement {
            corner: Corner::BottomLeft,
            output: Some("DP-2".into()),
        };
        let text = toml::to_string(&placement).unwrap();
        assert!(text.contains("bottom-left"));
        assert_eq!(toml::from_str::<Placement>(&text).unwrap(), placement);
        assert_eq!(
            toml::from_str::<Placement>("").unwrap(),
            Placement::default()
        );
    }
}
