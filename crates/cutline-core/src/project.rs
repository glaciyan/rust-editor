//! The timeline: clips laid end to end, each with parts that are kept or removed.

use crate::Timestamp;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Parts shorter than this are ignored and handles can't get closer.
pub const MIN_PART: Timestamp = Timestamp(100_000);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PartMode {
    /// The marked parts are what ends up in the export.
    #[default]
    Keep,
    /// The marked parts are cut out; everything else is exported.
    Remove,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub id: u64,
    pub start: Timestamp,
    pub end: Timestamp,
}

impl Part {
    pub fn len(&self) -> Timestamp {
        self.end - self.start
    }
    pub fn contains(&self, t: Timestamp) -> bool {
        t >= self.start && t <= self.end
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: u64,
    pub path: PathBuf,
    pub duration: Timestamp,
    /// Frame rate, used to snap handles to whole frames. 0 when unknown.
    #[serde(default)]
    pub fps: f64,
    /// Sorted by start, never overlapping.
    pub parts: Vec<Part>,
}

impl Clip {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "video".into())
    }
    pub fn stem(&self) -> String {
        self.path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "video".into())
    }
    fn snap(&self, t: Timestamp) -> Timestamp {
        t.snap_to_frame(self.fps).clamp(Timestamp::ZERO, self.duration)
    }
    fn sort(&mut self) {
        self.parts.sort_by_key(|p| p.start);
    }
    /// The free space around `t` not covered by any part.
    fn gap_at(&self, t: Timestamp) -> Option<(Timestamp, Timestamp)> {
        if self.parts.iter().any(|p| p.contains(t)) {
            return None;
        }
        let lo = self.parts.iter().filter(|p| p.end <= t).map(|p| p.end).max().unwrap_or(Timestamp::ZERO);
        let hi = self.parts.iter().filter(|p| p.start >= t).map(|p| p.start).min().unwrap_or(self.duration);
        Some((lo, hi))
    }
    fn gaps(&self) -> Vec<(Timestamp, Timestamp)> {
        let mut out = vec![];
        let mut cur = Timestamp::ZERO;
        for p in &self.parts {
            if p.start > cur {
                out.push((cur, p.start));
            }
            cur = cur.max(p.end);
        }
        if self.duration > cur {
            out.push((cur, self.duration));
        }
        out
    }
    fn neighbours(&self, idx: usize) -> (Timestamp, Timestamp) {
        let lo = if idx == 0 { Timestamp::ZERO } else { self.parts[idx - 1].end };
        let hi = self.parts.get(idx + 1).map(|p| p.start).unwrap_or(self.duration);
        (lo, hi)
    }
    /// The ranges of this clip that end up in the export.
    pub fn kept_ranges(&self, mode: PartMode) -> Vec<(Timestamp, Timestamp)> {
        let ranges = match mode {
            PartMode::Keep => self.parts.iter().map(|p| (p.start, p.end)).collect(),
            PartMode::Remove => self.gaps(),
        };
        ranges.into_iter().filter(|(a, b)| *b - *a >= MIN_PART).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartRef {
    pub clip: u64,
    pub part: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Start,
    End,
}

/// One piece of the output: a range of one source file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub clip: u64,
    pub path: PathBuf,
    pub start: Timestamp,
    pub end: Timestamp,
}

impl Segment {
    pub fn len(&self) -> Timestamp {
        self.end - self.start
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub clips: Vec<Clip>,
    pub mode: PartMode,
    pub selected: Option<PartRef>,
    next_id: u64,
}

impl Project {
    fn fresh_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    /// Append a video to the end of the timeline. In Keep mode the whole
    /// video starts as one part, so the two handles sit at its ends.
    pub fn add_clip(&mut self, path: PathBuf, duration: Timestamp, fps: f64) -> u64 {
        let id = self.fresh_id();
        let mut clip = Clip { id, path, duration, fps, parts: vec![] };
        if self.mode == PartMode::Keep {
            let pid = self.fresh_id();
            clip.parts.push(Part { id: pid, start: Timestamp::ZERO, end: duration });
            if self.selected.is_none() {
                self.selected = Some(PartRef { clip: id, part: pid });
            }
        }
        self.clips.push(clip);
        id
    }

    pub fn remove_clip(&mut self, clip: u64) {
        self.clips.retain(|c| c.id != clip);
        if self.selected.map(|s| s.clip) == Some(clip) {
            self.selected = None;
        }
    }

    /// Move a clip by `delta` places in the timeline.
    pub fn move_clip(&mut self, clip: u64, delta: isize) {
        if let Some(i) = self.clip_index(clip) {
            let j = (i as isize + delta).clamp(0, self.clips.len() as isize - 1) as usize;
            let c = self.clips.remove(i);
            self.clips.insert(j, c);
        }
    }

    pub fn clip(&self, id: u64) -> Option<&Clip> {
        self.clips.iter().find(|c| c.id == id)
    }
    pub fn clip_mut(&mut self, id: u64) -> Option<&mut Clip> {
        self.clips.iter_mut().find(|c| c.id == id)
    }
    pub fn clip_index(&self, id: u64) -> Option<usize> {
        self.clips.iter().position(|c| c.id == id)
    }
    pub fn part(&self, r: PartRef) -> Option<&Part> {
        self.clip(r.clip)?.parts.iter().find(|p| p.id == r.part)
    }

    pub fn total_duration(&self) -> Timestamp {
        Timestamp(self.clips.iter().map(|c| c.duration.0).sum())
    }

    /// Where a clip starts on the overall timeline.
    pub fn clip_offset(&self, id: u64) -> Timestamp {
        let mut t = Timestamp::ZERO;
        for c in &self.clips {
            if c.id == id {
                return t;
            }
            t = t + c.duration;
        }
        t
    }

    /// Map a timeline position to (clip id, time inside that clip).
    pub fn locate(&self, global: Timestamp) -> Option<(u64, Timestamp)> {
        let mut t = Timestamp::ZERO;
        for (i, c) in self.clips.iter().enumerate() {
            let last = i + 1 == self.clips.len();
            if global < t + c.duration || last {
                return Some((c.id, (global - t).clamp(Timestamp::ZERO, c.duration)));
            }
            t = t + c.duration;
        }
        None
    }

    pub fn set_mode(&mut self, mode: PartMode) {
        self.mode = mode;
    }

    /// Add a part at `at` (clip-local). Uses the gap under `at`, or the
    /// biggest free gap if `at` is already inside a part.
    pub fn add_part(&mut self, clip: u64, at: Timestamp) -> Option<PartRef> {
        let pid = self.next_id + 1;
        let c = self.clip_mut(clip)?;
        let (gap, centre) = match c.gap_at(at) {
            Some(g) => (g, at),
            None => {
                let g = c.gaps().into_iter().max_by_key(|(a, b)| (*b - *a).0)?;
                (g, Timestamp((g.0 .0 + g.1 .0) / 2))
            }
        };
        let room = gap.1 - gap.0;
        if room < MIN_PART {
            return None;
        }
        let want = Timestamp((c.duration.0 / 10).max(2_000_000)).min(room);
        let mut start = Timestamp(centre.0 - want.0 / 2).clamp(gap.0, gap.1 - want);
        let mut end = start + want;
        start = c.snap(start).max(gap.0);
        end = c.snap(end).min(gap.1);
        if end - start < MIN_PART {
            return None;
        }
        c.parts.push(Part { id: pid, start, end });
        c.sort();
        self.next_id = pid;
        let r = PartRef { clip, part: pid };
        self.selected = Some(r);
        Some(r)
    }

    pub fn remove_part(&mut self, r: PartRef) {
        if let Some(c) = self.clip_mut(r.clip) {
            c.parts.retain(|p| p.id != r.part);
        }
        if self.selected == Some(r) {
            self.selected = None;
        }
    }

    /// Drag a handle to `t` (clip-local). Handles stop at the neighbouring
    /// parts and keep at least [`MIN_PART`] between them. Returns where the
    /// handle actually landed.
    pub fn move_handle(&mut self, r: PartRef, h: Handle, t: Timestamp) -> Option<Timestamp> {
        let c = self.clip_mut(r.clip)?;
        let idx = c.parts.iter().position(|p| p.id == r.part)?;
        let (lo, hi) = c.neighbours(idx);
        let t = c.snap(t);
        let p = &mut c.parts[idx];
        let landed = match h {
            Handle::Start => {
                p.start = t.clamp(lo, p.end - MIN_PART);
                p.start
            }
            Handle::End => {
                p.end = t.clamp(p.start + MIN_PART, hi);
                p.end
            }
        };
        Some(landed)
    }

    /// Move a whole part by `delta`, keeping its length.
    pub fn shift_part(&mut self, r: PartRef, delta: Timestamp) {
        let Some(c) = self.clip_mut(r.clip) else { return };
        let Some(idx) = c.parts.iter().position(|p| p.id == r.part) else { return };
        let (lo, hi) = c.neighbours(idx);
        let len = c.parts[idx].len();
        let start = (c.parts[idx].start + delta).clamp(lo, hi - len);
        let start = c.snap(start).clamp(lo, hi - len);
        c.parts[idx].start = start;
        c.parts[idx].end = start + len;
    }

    /// The part a key press at `t` should act on: the one under `t`, else
    /// the selected one in the same clip.
    fn active_part(&self, clip: u64, t: Timestamp) -> Option<PartRef> {
        let c = self.clip(clip)?;
        if let Some(p) = c.parts.iter().find(|p| p.contains(t)) {
            return Some(PartRef { clip, part: p.id });
        }
        self.selected.filter(|s| s.clip == clip && c.parts.iter().any(|p| p.id == s.part))
    }

    /// `I`: start the active part at `t`, or begin a new part there.
    pub fn set_in(&mut self, clip: u64, t: Timestamp) -> Option<PartRef> {
        if let Some(r) = self.active_part(clip, t) {
            let p = self.part(r)?.clone();
            if t < p.end - MIN_PART {
                let c = self.clip(clip)?;
                let idx = c.parts.iter().position(|q| q.id == p.id)?;
                let (lo, _) = c.neighbours(idx);
                if t >= lo {
                    self.move_handle(r, Handle::Start, t);
                    self.selected = Some(r);
                    return Some(r);
                }
            }
        }
        // No part to adjust: open a new one from t to the next part (or the end).
        let pid = self.next_id + 1;
        let c = self.clip_mut(clip)?;
        let t = c.snap(t);
        let (lo, hi) = c.gap_at(t)?;
        if hi - t < MIN_PART || t < lo {
            return None;
        }
        c.parts.push(Part { id: pid, start: t, end: hi });
        c.sort();
        self.next_id = pid;
        let r = PartRef { clip, part: pid };
        self.selected = Some(r);
        Some(r)
    }

    /// `O`: end the active part at `t`, or close a new part there.
    pub fn set_out(&mut self, clip: u64, t: Timestamp) -> Option<PartRef> {
        if let Some(r) = self.active_part(clip, t) {
            let p = self.part(r)?.clone();
            if t > p.start + MIN_PART {
                let c = self.clip(clip)?;
                let idx = c.parts.iter().position(|q| q.id == p.id)?;
                let (_, hi) = c.neighbours(idx);
                if t <= hi {
                    self.move_handle(r, Handle::End, t);
                    self.selected = Some(r);
                    return Some(r);
                }
            }
        }
        let pid = self.next_id + 1;
        let c = self.clip_mut(clip)?;
        let t = c.snap(t);
        let (lo, hi) = c.gap_at(t)?;
        if t - lo < MIN_PART || t > hi {
            return None;
        }
        c.parts.push(Part { id: pid, start: lo, end: t });
        c.sort();
        self.next_id = pid;
        let r = PartRef { clip, part: pid };
        self.selected = Some(r);
        Some(r)
    }

    /// What will be exported, in timeline order.
    pub fn segments(&self) -> Vec<Segment> {
        let mut out = vec![];
        for c in &self.clips {
            for (start, end) in c.kept_ranges(self.mode) {
                out.push(Segment { clip: c.id, path: c.path.clone(), start, end });
            }
        }
        out
    }

    pub fn output_duration(&self) -> Timestamp {
        Timestamp(self.segments().iter().map(|s| s.len().0).sum())
    }

    /// The next kept range at or after `t` in clip `clip`, searching later
    /// clips too. Used by playback to skip what's cut out.
    pub fn next_kept(&self, clip: u64, t: Timestamp) -> Option<(u64, Timestamp)> {
        let start = self.clip_index(clip)?;
        for (i, c) in self.clips.iter().enumerate().skip(start) {
            for (a, b) in c.kept_ranges(self.mode) {
                if i == start && b <= t {
                    continue;
                }
                return Some((c.id, if i == start { a.max(t) } else { a }));
            }
        }
        None
    }

    /// Is `t` (clip-local) inside something that will be exported?
    pub fn is_kept(&self, clip: u64, t: Timestamp) -> bool {
        self.clip(clip)
            .map(|c| c.kept_ranges(self.mode).iter().any(|(a, b)| t >= *a && t < *b))
            .unwrap_or(false)
    }

    /// Fix up anything a hand-edited or old session file might get wrong.
    pub fn normalise(&mut self) {
        let mut max_id = self.next_id;
        for c in &mut self.clips {
            max_id = max_id.max(c.id);
            for p in &mut c.parts {
                p.start = p.start.clamp(Timestamp::ZERO, c.duration);
                p.end = p.end.clamp(p.start, c.duration);
                max_id = max_id.max(p.id);
            }
            c.parts.retain(|p| p.len() >= MIN_PART);
            c.sort();
            let mut last_end = Timestamp::ZERO;
            for p in &mut c.parts {
                if p.start < last_end {
                    p.start = last_end;
                }
                if p.end < p.start {
                    p.end = p.start;
                }
                last_end = p.end;
            }
            c.parts.retain(|p| p.len() >= MIN_PART);
        }
        self.next_id = max_id;
        if let Some(s) = self.selected {
            if self.part(s).is_none() {
                self.selected = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: f64) -> Timestamp {
        Timestamp::from_secs(s)
    }

    fn one_clip() -> (Project, u64) {
        let mut p = Project::default();
        let id = p.add_clip("/v/a.mp4".into(), ts(100.0), 0.0);
        (p, id)
    }

    #[test]
    fn new_clip_is_fully_kept() {
        let (p, id) = one_clip();
        let segs = p.segments();
        assert_eq!(segs.len(), 1);
        assert_eq!((segs[0].clip, segs[0].start, segs[0].end), (id, ts(0.0), ts(100.0)));
    }

    #[test]
    fn handles_respect_neighbours() {
        let (mut p, id) = one_clip();
        let first = p.selected.unwrap();
        p.move_handle(first, Handle::End, ts(30.0));
        let second = p.add_part(id, ts(60.0)).unwrap();
        // Can't drag the second part's start into the first part.
        let landed = p.move_handle(second, Handle::Start, ts(10.0)).unwrap();
        assert_eq!(landed, ts(30.0));
        // Can't cross its own end.
        let end = p.part(second).unwrap().end;
        let landed = p.move_handle(second, Handle::Start, end + ts(5.0)).unwrap();
        assert_eq!(landed, end - MIN_PART);
    }

    #[test]
    fn remove_mode_inverts() {
        let (mut p, id) = one_clip();
        let r = p.selected.unwrap();
        p.move_handle(r, Handle::Start, ts(10.0));
        p.move_handle(r, Handle::End, ts(20.0));
        p.set_mode(PartMode::Remove);
        let segs = p.segments();
        assert_eq!(segs.len(), 2);
        assert_eq!((segs[0].start, segs[0].end), (ts(0.0), ts(10.0)));
        assert_eq!((segs[1].start, segs[1].end), (ts(20.0), ts(100.0)));
        assert!(!p.is_kept(id, ts(15.0)));
        assert_eq!(p.next_kept(id, ts(15.0)), Some((id, ts(20.0))));
    }

    #[test]
    fn in_out_keys() {
        let (mut p, id) = one_clip();
        p.set_in(id, ts(12.0));
        p.set_out(id, ts(40.0));
        let segs = p.segments();
        assert_eq!((segs[0].start, segs[0].end), (ts(12.0), ts(40.0)));
        // I outside any part starts a new part running to the end.
        p.selected = None;
        p.set_in(id, ts(70.0));
        let segs = p.segments();
        assert_eq!(segs.len(), 2);
        assert_eq!((segs[1].start, segs[1].end), (ts(70.0), ts(100.0)));
    }

    #[test]
    fn timeline_mapping() {
        let mut p = Project::default();
        let a = p.add_clip("a.mp4".into(), ts(10.0), 0.0);
        let b = p.add_clip("b.mp4".into(), ts(5.0), 0.0);
        assert_eq!(p.locate(ts(3.0)), Some((a, ts(3.0))));
        assert_eq!(p.locate(ts(12.0)), Some((b, ts(2.0))));
        assert_eq!(p.locate(ts(99.0)), Some((b, ts(5.0))));
        assert_eq!(p.clip_offset(b), ts(10.0));
        p.move_clip(b, -1);
        assert_eq!(p.clips[0].id, b);
    }

    #[test]
    fn snapping_to_frames() {
        let mut p = Project::default();
        let id = p.add_clip("a.mp4".into(), ts(10.0), 25.0);
        let r = p.selected.unwrap();
        let landed = p.move_handle(r, Handle::Start, ts(1.019)).unwrap();
        assert_eq!(landed, ts(1.0));
        let _ = id;
    }
}
