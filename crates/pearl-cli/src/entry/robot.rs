//! The owner's spinning ASCII robot above the wordmark: 36 frames, 44 x 24, 12 fps.
//! Made by Rushu / Weazel VFX (frames rendered from the 3D robot model; source
//! `ascii-robot.js` / `frames.json`, copied as assets/robot-frames.json).
//!
//! The frames are plain ASCII. [`Robot::half`] makes a half-size copy (22 x 12) for
//! terminals too short for the full one: each 2 x 2 block keeps its densest character.

use anyhow::{anyhow, ensure, Result};
use serde_json::Value;

/// The frames as shipped, embedded at build time.
pub const FRAMES_JSON: &str = include_str!("../../assets/robot-frames.json");

/// Characters from light to dense (every character the frames use).
pub const RAMP: &str = " .,:;-=+*#%@";

#[derive(Debug, Clone)]
pub struct Robot {
    pub cols: usize,
    pub rows: usize,
    pub fps: u32,
    /// frames[i][row] is one line, right-padded to `cols`.
    pub frames: Vec<Vec<String>>,
}

impl Robot {
    /// Parse `{cols, rows, fps, frames: [string]}`: every frame becomes exactly `rows`
    /// lines of exactly `cols` characters (short lines padded; nothing is cut, a longer
    /// line is an error).
    pub fn parse(json: &str) -> Result<Robot> {
        let v: Value = serde_json::from_str(json)?;
        let n = |k: &str| v[k].as_u64().ok_or_else(|| anyhow!("robot frames: no {k}"));
        let (cols, rows, fps) = (n("cols")? as usize, n("rows")? as usize, n("fps")? as u32);
        ensure!(cols > 0 && rows > 0 && fps > 0, "robot frames: empty size");
        let list = v["frames"].as_array().ok_or_else(|| anyhow!("robot frames: no frames"))?;
        ensure!(!list.is_empty(), "robot frames: no frames");
        let mut frames = Vec::with_capacity(list.len());
        for (i, f) in list.iter().enumerate() {
            let s = f.as_str().ok_or_else(|| anyhow!("robot frame {i} is not a string"))?;
            ensure!(s.chars().all(|c| c == '\n' || (' '..='~').contains(&c)), "robot frame {i}: not plain ASCII");
            let mut lines: Vec<String> = s.split('\n').map(String::from).collect();
            ensure!(lines.len() <= rows, "robot frame {i}: {} lines > {rows}", lines.len());
            for l in &lines {
                ensure!(l.len() <= cols, "robot frame {i}: a line of {} > {cols}", l.len());
            }
            lines.resize(rows, String::new());
            for l in lines.iter_mut() {
                *l = format!("{l:<cols$}");
            }
            frames.push(lines);
        }
        Ok(Robot { cols, rows, fps, frames })
    }

    /// The embedded robot.
    pub fn embedded() -> Robot {
        Robot::parse(FRAMES_JSON).expect("assets/robot-frames.json is valid")
    }

    /// Frame for a time since start (loops).
    pub fn frame_at(&self, elapsed: std::time::Duration) -> &[String] {
        let i = (elapsed.as_millis() as u64 * self.fps as u64 / 1000) as usize % self.frames.len();
        &self.frames[i]
    }

    /// Half size: each 2 x 2 block becomes its densest character.
    pub fn half(&self) -> Robot {
        let dens = |c: char| RAMP.find(c).unwrap_or(0);
        let (cols, rows) = (self.cols.div_ceil(2), self.rows.div_ceil(2));
        let frames = self
            .frames
            .iter()
            .map(|f| {
                let g: Vec<Vec<char>> = f.iter().map(|l| l.chars().collect()).collect();
                (0..rows)
                    .map(|r| {
                        (0..cols)
                            .map(|c| {
                                let mut best = ' ';
                                for (dr, dc) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                                    if let Some(&ch) = g.get(2 * r + dr).and_then(|l| l.get(2 * c + dc)) {
                                        if dens(ch) > dens(best) {
                                            best = ch;
                                        }
                                    }
                                }
                                best
                            })
                            .collect()
                    })
                    .collect()
            })
            .collect();
        Robot { cols, rows, fps: self.fps, frames }
    }
}

/// Shade of one robot character: 0 = brightest .. 3 = dimmest (see theme.rs).
pub fn shade(c: char) -> u8 {
    match c {
        '@' | '%' => 0,
        '#' | '*' => 1,
        '+' | '=' | '-' => 2,
        _ => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_frames_load_and_size() {
        let r = Robot::embedded();
        assert_eq!((r.cols, r.rows, r.fps, r.frames.len()), (44, 24, 12, 36));
        for f in &r.frames {
            assert_eq!(f.len(), 24);
            assert!(f.iter().all(|l| l.chars().count() == 44));
        }
        // the frames are the shipped ones: the first frame's top row
        let src: Value = serde_json::from_str(FRAMES_JSON).unwrap();
        let first = src["frames"][0].as_str().unwrap().split('\n').next().unwrap();
        assert_eq!(r.frames[0][0].trim_end(), first.trim_end());
    }

    #[test]
    fn loops_at_its_fps() {
        let r = Robot::embedded();
        let ms = |m| std::time::Duration::from_millis(m);
        assert_eq!(r.frame_at(ms(0)), &r.frames[0][..]);
        assert_eq!(r.frame_at(ms(1000 / 12 + 1)), &r.frames[1][..]);
        // 36 frames at 12 fps = 3 s per turn
        assert_eq!(r.frame_at(ms(3000)), &r.frames[0][..]);
    }

    #[test]
    fn half_size_keeps_the_shape() {
        let r = Robot::embedded();
        let h = r.half();
        assert_eq!((h.cols, h.rows, h.frames.len()), (22, 12, 36));
        assert!(h.frames.iter().all(|f| f.len() == 12 && f.iter().all(|l| l.chars().count() == 22)));
        // nothing invented: every character of the half frame occurs in the full frame
        for (a, b) in r.frames.iter().zip(&h.frames) {
            let full: String = a.concat();
            assert!(b.concat().chars().all(|c| full.contains(c)));
        }
    }

    #[test]
    fn rejects_bad_frames() {
        assert!(Robot::parse(r#"{"cols":4,"rows":1,"fps":12,"frames":["abcde"]}"#).is_err());
        assert!(Robot::parse(r#"{"cols":4,"rows":1,"fps":12,"frames":["a\nb"]}"#).is_err());
        assert!(Robot::parse("{\"cols\":4,\"rows\":1,\"fps\":12,\"frames\":[\"a\\u001b\"]}").is_err());
        assert!(Robot::parse(r#"{"cols":4,"rows":1,"fps":12,"frames":[]}"#).is_err());
        let ok = Robot::parse(r#"{"cols":4,"rows":2,"fps":12,"frames":["ab"]}"#).unwrap();
        assert_eq!(ok.frames[0], vec!["ab  ".to_string(), "    ".to_string()]);
    }
}
