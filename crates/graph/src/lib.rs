//! sluice-graph — commit graph layout (02 §4): lane assignment with the classic
//! "active lanes" sweep, straight-segment edge routing between adjacent rows,
//! and stable colors hashed from the ref / tip that opened a lane.
//!
//! The input is the log in display order (children before parents — which
//! `--date-order` / `--topo-order` guarantee). The output is one [`RowLayout`]
//! per row plus the edge segments that leave that row towards the next one.
//! A renderer draws, for row `i`, the lower half of `rows[i].out_edges` and the
//! upper half of `rows[i-1].out_edges`, so every stroke stays inside its row.

use std::collections::HashMap;

use sluice_core::Oid;

/// Palette size the color index is taken modulo of (the prototype uses 3 inks:
/// cyan / magenta / process-yellow; dark theme reuses the same indices).
pub const PALETTE: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    /// Lane at this row's center.
    pub from_lane: u16,
    /// Lane at the next row's center.
    pub to_lane: u16,
    pub color: u16,
}

#[derive(Clone, Debug, Default)]
pub struct RowLayout {
    /// Lane the commit dot sits in.
    pub lane: u16,
    pub color: u16,
    /// Segments from this row's center down to the next row's center.
    pub out_edges: Vec<Edge>,
}

#[derive(Clone, Debug, Default)]
pub struct GraphLayout {
    pub rows: Vec<RowLayout>,
    /// Highest lane index + 1 seen anywhere (for column width).
    pub max_lanes: u16,
}

/// A commit as the layout needs it.
pub struct Node<'a> {
    pub id: &'a Oid,
    pub parents: &'a [Oid],
    /// Ref name that makes this commit a tip (used for stable colors), if any.
    pub tip_ref: Option<&'a str>,
}

#[derive(Clone, Debug)]
struct Lane {
    /// Commit id expected next in this lane.
    expecting: Oid,
    color: u16,
}

fn stable_color(seed: &str) -> u16 {
    // FNV-1a — the same seed (branch name, else tip id) always gets the same ink.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in seed.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (h % PALETTE as u64) as u16
}

pub fn layout<'a>(nodes: impl IntoIterator<Item = Node<'a>>) -> GraphLayout {
    let nodes: Vec<Node<'a>> = nodes.into_iter().collect();
    let mut rows: Vec<RowLayout> = Vec::with_capacity(nodes.len());
    let mut active: Vec<Option<Lane>> = Vec::new();
    let mut max_lanes: u16 = 0;
    // Rows already emitted, so a parent that (due to clock skew) appears
    // *above* its child never opens a dangling lane.
    let mut seen: HashMap<&Oid, usize> = HashMap::with_capacity(nodes.len());

    for (row_ix, node) in nodes.iter().enumerate() {
        seen.insert(node.id, row_ix);

        // 1. Which lane is waiting for this commit? (first match = its lane; other matches merge into it)
        let mut my_lane: Option<usize> = None;
        let mut merged_in: Vec<usize> = Vec::new();
        for (ix, lane) in active.iter().enumerate() {
            if let Some(l) = lane
                && &l.expecting == node.id
            {
                if my_lane.is_none() {
                    my_lane = Some(ix);
                } else {
                    merged_in.push(ix);
                }
            }
        }
        let my_lane = match my_lane {
            Some(ix) => ix,
            None => {
                // A new tip: take the first free slot (or append).
                let seed = node
                    .tip_ref
                    .map(str::to_owned)
                    .unwrap_or_else(|| node.id.to_string());
                let lane = Lane {
                    expecting: node.id.clone(),
                    color: stable_color(&seed),
                };
                match active.iter().position(Option::is_none) {
                    Some(ix) => {
                        active[ix] = Some(lane);
                        ix
                    }
                    None => {
                        active.push(Some(lane));
                        active.len() - 1
                    }
                }
            }
        };
        let my_color = active[my_lane].as_ref().map(|l| l.color).unwrap_or_default();
        for ix in &merged_in {
            active[*ix] = None; // those lanes ended at this commit
        }

        // 2. Hand the lane on to parents.
        let mut out_edges: Vec<Edge> = Vec::new();
        let mut parents = node.parents.iter().filter(|p| !seen.contains_key(*p));
        let first_parent = parents.next();
        let mut lane_after: Vec<Option<Lane>> = active.clone();

        match first_parent {
            None => {
                lane_after[my_lane] = None; // root (or parent already drawn above): lane ends here
            }
            Some(p0) => {
                // If some other lane already expects p0, we merge into it and free ours.
                if let Some(other) = active
                    .iter()
                    .position(|l| l.as_ref().is_some_and(|l| &l.expecting == p0))
                    && other != my_lane
                {
                    let color = my_color;
                    out_edges.push(Edge {
                        from_lane: my_lane as u16,
                        to_lane: other as u16,
                        color,
                    });
                    lane_after[my_lane] = None;
                } else {
                    lane_after[my_lane] = Some(Lane {
                        expecting: p0.clone(),
                        color: my_color,
                    });
                    out_edges.push(Edge {
                        from_lane: my_lane as u16,
                        to_lane: my_lane as u16,
                        color: my_color,
                    });
                }
            }
        }
        for p in parents {
            if let Some(other) = lane_after
                .iter()
                .position(|l| l.as_ref().is_some_and(|l| &l.expecting == p))
            {
                let color = lane_after[other].as_ref().map(|l| l.color).unwrap_or(my_color);
                out_edges.push(Edge {
                    from_lane: my_lane as u16,
                    to_lane: other as u16,
                    color,
                });
            } else {
                let color = stable_color(p.as_str());
                let lane = Lane {
                    expecting: p.clone(),
                    color,
                };
                let ix = match lane_after.iter().position(Option::is_none) {
                    Some(ix) => {
                        lane_after[ix] = Some(lane);
                        ix
                    }
                    None => {
                        lane_after.push(Some(lane));
                        lane_after.len() - 1
                    }
                };
                out_edges.push(Edge {
                    from_lane: my_lane as u16,
                    to_lane: ix as u16,
                    color,
                });
            }
        }
        // 3. Pass-through lanes (not touched by this commit) continue straight down.
        for (ix, lane) in lane_after.iter().enumerate() {
            if ix == my_lane {
                continue;
            }
            if let Some(l) = lane
                && active
                    .get(ix)
                    .is_some_and(|a| a.as_ref().is_some_and(|a| a.expecting == l.expecting))
            {
                out_edges.push(Edge {
                    from_lane: ix as u16,
                    to_lane: ix as u16,
                    color: l.color,
                });
            }
        }
        // Trim trailing empty lanes so max_lanes stays tight.
        while lane_after.last().is_some_and(Option::is_none) {
            lane_after.pop();
        }
        active = lane_after;
        max_lanes = max_lanes.max((my_lane as u16) + 1).max(active.len() as u16);
        rows.push(RowLayout {
            lane: my_lane as u16,
            color: my_color,
            out_edges,
        });
    }
    GraphLayout {
        rows,
        max_lanes: max_lanes.max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(s: &str) -> Oid {
        Oid::new(s)
    }

    #[test]
    fn linear_history_is_one_lane() {
        let ids: Vec<Oid> = (0..4).map(|i| oid(&format!("c{i}"))).collect();
        let parents: Vec<Vec<Oid>> = (0..4)
            .map(|i| if i < 3 { vec![ids[i + 1].clone()] } else { vec![] })
            .collect();
        let nodes = ids.iter().zip(parents.iter()).map(|(id, p)| Node {
            id,
            parents: p,
            tip_ref: None,
        });
        let g = layout(nodes);
        assert_eq!(g.max_lanes, 1);
        assert!(g.rows.iter().all(|r| r.lane == 0));
        assert_eq!(g.rows[0].out_edges.len(), 1);
        assert!(g.rows[3].out_edges.is_empty());
    }

    #[test]
    fn merge_opens_and_closes_a_second_lane() {
        // m (merge of a and b) -> a -> base ; b -> base
        let m = oid("m");
        let a = oid("a");
        let b = oid("b");
        let base = oid("base");
        let pm = vec![a.clone(), b.clone()];
        let pa = vec![base.clone()];
        let pb = vec![base.clone()];
        let pbase: Vec<Oid> = vec![];
        let nodes = vec![
            Node {
                id: &m,
                parents: &pm,
                tip_ref: Some("main"),
            },
            Node {
                id: &a,
                parents: &pa,
                tip_ref: None,
            },
            Node {
                id: &b,
                parents: &pb,
                tip_ref: None,
            },
            Node {
                id: &base,
                parents: &pbase,
                tip_ref: None,
            },
        ];
        let g = layout(nodes);
        assert_eq!(g.max_lanes, 2);
        assert_eq!(g.rows[0].lane, 0);
        assert_eq!(g.rows[1].lane, 0);
        assert_eq!(g.rows[2].lane, 1);
        assert_eq!(g.rows[3].lane, 0);
        // merge row fans out to both lanes
        assert_eq!(g.rows[0].out_edges.len(), 2);
        // b merges back into lane 0 at base
        assert!(
            g.rows[2]
                .out_edges
                .iter()
                .any(|e| e.from_lane == 1 && e.to_lane == 0)
        );
    }
}
