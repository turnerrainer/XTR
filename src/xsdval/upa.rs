//! Unique particle attribution (UPA), simplified.
//!
//! At every decision point of a content model, the elements that can
//! come next must have different qualified names. The check computes
//! the follow set of every element particle. The follow set crosses
//! sequence and extension boundaries (the extension base is the first
//! item of a merged sequence), passes groups that can be empty, and
//! includes the first set of a group that repeats.
//!
//! The check is stricter than XSD for particles with counters: an
//! element with `maxOccurs` above 1 can follow itself. A model that
//! this check refuses is refused at boot (fail closed).

use super::model::Particle;
use crate::inbound::contract::QName;
use std::collections::BTreeSet;

struct Info {
    nullable: bool,
    first: BTreeSet<usize>,
    last: BTreeSet<usize>,
}

struct State<'a> {
    names: Vec<&'a QName>,
    follow: Vec<BTreeSet<usize>>,
}

/// True when no decision point of `p` has two different particles with
/// the same qualified name.
pub(super) fn is_deterministic(p: &Particle) -> bool {
    let mut st = State {
        names: Vec::new(),
        follow: Vec::new(),
    };
    let info = analyse(p, &mut st);
    let distinct = |set: &BTreeSet<usize>| {
        let mut seen = BTreeSet::new();
        set.iter().all(|&i| seen.insert(st.names[i]))
    };
    distinct(&info.first) && st.follow.iter().all(distinct)
}

fn repeats(max: Option<u32>) -> bool {
    max.is_none_or(|m| m > 1)
}

fn analyse<'a>(p: &'a Particle, st: &mut State<'a>) -> Info {
    match p {
        Particle::Element { decl, occurs } => {
            let id = st.names.len();
            st.names.push(&decl.name);
            st.follow.push(BTreeSet::new());
            if repeats(occurs.max) {
                st.follow[id].insert(id);
            }
            Info {
                nullable: occurs.min == 0,
                first: BTreeSet::from([id]),
                last: BTreeSet::from([id]),
            }
        }
        Particle::Sequence { items, occurs } => {
            let parts: Vec<Info> = items.iter().map(|i| analyse(i, st)).collect();
            for k in 0..parts.len() {
                for j in k + 1..parts.len() {
                    for &l in &parts[k].last {
                        st.follow[l].extend(parts[j].first.iter().copied());
                    }
                    if !parts[j].nullable {
                        break;
                    }
                }
            }
            let mut first = BTreeSet::new();
            for part in &parts {
                first.extend(part.first.iter().copied());
                if !part.nullable {
                    break;
                }
            }
            let mut last = BTreeSet::new();
            for part in parts.iter().rev() {
                last.extend(part.last.iter().copied());
                if !part.nullable {
                    break;
                }
            }
            let nullable = parts.iter().all(|p| p.nullable);
            finish(st, nullable, first, last, occurs.min, occurs.max)
        }
        Particle::Choice { items, occurs } => {
            let parts: Vec<Info> = items.iter().map(|i| analyse(i, st)).collect();
            let mut first = BTreeSet::new();
            let mut last = BTreeSet::new();
            for part in &parts {
                first.extend(part.first.iter().copied());
                last.extend(part.last.iter().copied());
            }
            let nullable = parts.iter().any(|p| p.nullable);
            finish(st, nullable, first, last, occurs.min, occurs.max)
        }
    }
}

/// Applies the occurrence of a group: a repeat links its last set to its
/// first set, and `minOccurs=0` makes the group nullable.
fn finish(
    st: &mut State<'_>,
    nullable: bool,
    first: BTreeSet<usize>,
    last: BTreeSet<usize>,
    min: u32,
    max: Option<u32>,
) -> Info {
    if repeats(max) {
        for &l in &last {
            st.follow[l].extend(first.iter().copied());
        }
    }
    Info {
        nullable: nullable || min == 0,
        first,
        last,
    }
}
