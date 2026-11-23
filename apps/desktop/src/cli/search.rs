use super::{CliError, ErrorKind, SearchScope};
use crate::model::{Block, Note};
use unicode_normalization::UnicodeNormalization;

pub(super) struct Query {
    terms: Vec<Vec<char>>,
    exact: bool,
    scope: SearchScope,
}

type Rank = (u8, usize, usize, usize, usize);

pub(super) struct Hit {
    pub rank: Rank,
    pub field: &'static str,
    pub snippet: String,
}

struct Field<'a> {
    kind: &'static str,
    scope: SearchScope,
    priority: usize,
    text: &'a str,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct Match {
    strength: u8,
    field: usize,
    extra: usize,
    start: usize,
    end: usize,
}

impl Query {
    pub fn new(input: &str, exact: bool, scope: SearchScope) -> Result<Self, CliError> {
        if input.len() > 1024 {
            return Err(CliError::new(ErrorKind::Input, "搜索词过长"));
        }
        let folded = normalize(input).0.into_iter().collect::<String>();
        let terms: Vec<Vec<char>> = if exact {
            vec![folded.chars().collect()]
        } else {
            folded
                .split_whitespace()
                .map(|term| term.chars().collect())
                .collect()
        };
        if terms.is_empty()
            || terms[0].is_empty()
            || terms.len() > 16
            || terms.iter().map(Vec::len).sum::<usize>() > 256
        {
            return Err(CliError::new(
                ErrorKind::Input,
                "请输入搜索词，最多 16 个词且共 256 字符",
            ));
        }
        Ok(Self {
            terms,
            exact,
            scope,
        })
    }

    pub fn find(&self, note: &Note) -> Option<Hit> {
        let fields: Vec<_> = std::iter::once(Field {
            kind: "title",
            scope: SearchScope::Title,
            priority: 0,
            text: &note.title,
        })
        .chain(note.blocks.iter().filter_map(|block| match block {
            Block::Markdown { text, .. } => Some(Field {
                kind: "body",
                scope: SearchScope::Body,
                priority: 1,
                text,
            }),
            Block::Audio {
                text: Some(text), ..
            } => Some(Field {
                kind: "transcript",
                scope: SearchScope::Transcript,
                priority: 2,
                text,
            }),
            _ => None,
        }))
        .filter(|field| self.scope == SearchScope::All || self.scope == field.scope)
        .collect();
        let mut best = vec![None; self.terms.len()];
        let mut closest = None;
        for (index, field) in fields.iter().enumerate() {
            let (chars, _) = normalize(field.text);
            let mut local = Vec::new();
            for (term_index, term) in self.terms.iter().enumerate() {
                let Some(matched) = find_match(&chars, term, self.exact, index) else {
                    continue;
                };
                let rank = (matched.strength, field.priority, matched.extra, matched);
                let previous = &mut best[term_index];
                if previous.is_none_or(|previous| rank < previous) {
                    *previous = Some(rank);
                }
                local.push(matched);
            }
            if local.len() == self.terms.len() {
                let start = local.iter().map(|m| m.start).min()?;
                let end = local.iter().map(|m| m.end).max()?;
                let max_strength = local.iter().map(|m| m.strength).max()?;
                let sum_strength = local.iter().map(|m| usize::from(m.strength)).sum::<usize>();
                let candidate = (max_strength, sum_strength, end - start);
                if closest.is_none_or(|previous| candidate < previous) {
                    closest = Some(candidate);
                }
            }
        }
        let best: Vec<_> = best.into_iter().collect::<Option<_>>()?;
        let max_strength = best.iter().map(|entry| entry.0).max()?;
        let sum_strength = best.iter().map(|entry| usize::from(entry.0)).sum();
        let field_priority = best.iter().map(|entry| entry.1).sum();
        let extra = best.iter().map(|entry| entry.2).sum();
        let proximity = closest
            .filter(|&(max, sum, _)| (max, sum) == (max_strength, sum_strength))
            .map_or(usize::MAX, |(_, _, span)| span);
        let matched = best.iter().min()?.3;
        let field = &fields[matched.field];
        Some(Hit {
            rank: (max_strength, sum_strength, field_priority, proximity, extra),
            field: field.kind,
            snippet: snippet(field.text, matched),
        })
    }
}

// Store original byte offsets so the snippet preserves the user's spelling and width.
fn normalize(text: &str) -> (Vec<char>, Vec<usize>) {
    let mut chars = Vec::new();
    let mut offsets = Vec::new();
    for (offset, original) in text.char_indices() {
        for ch in std::iter::once(original)
            .nfkc()
            .flat_map(char::to_lowercase)
        {
            let ch = if ch.is_whitespace() {
                if chars.is_empty() || chars.last() == Some(&' ') {
                    continue;
                }
                ' '
            } else {
                ch
            };
            chars.push(ch);
            offsets.push(offset);
        }
    }
    if chars.last() == Some(&' ') {
        chars.pop();
        offsets.pop();
    }
    (chars, offsets)
}

fn find_match(text: &[char], term: &[char], exact: bool, field: usize) -> Option<Match> {
    let build = |strength, extra, start, end| Match {
        strength,
        field,
        extra,
        start,
        end,
    };
    if let Some(start) = text.windows(term.len()).position(|window| window == term) {
        return Some(build(0, 0, start, start + term.len()));
    }
    if exact || term.len() < 2 {
        return None;
    }

    let allowed_skip = (term.len() / 2 + 1).min(8);
    let skipped = text
        .iter()
        .enumerate()
        .filter(|&(_, ch)| *ch == term[0])
        .filter_map(|(start, _)| {
            let mut rest = text
                .iter()
                .enumerate()
                .skip(start + 1)
                .take(term.len() + allowed_skip - 1);
            let mut end = start + 1;
            for expected in &term[1..] {
                end = rest.find(|&(_, ch)| ch == expected)?.0 + 1;
            }
            Some((end - start - term.len(), start, end))
        })
        .min();
    if let Some((extra, start, end)) = skipped {
        return Some(build(1, extra, start, end));
    }
    if !(4..=32).contains(&term.len()) {
        return None;
    }

    [term.len(), term.len() + 1, term.len() - 1]
        .into_iter()
        .find_map(|size| {
            let start = text
                .windows(size)
                .position(|window| one_edit(window, term))?;
            Some(build(2, 1, start, start + size))
        })
}

fn one_edit(left: &[char], right: &[char]) -> bool {
    let (shorter, longer) = if left.len() <= right.len() {
        (left, right)
    } else {
        (right, left)
    };
    let mismatch = shorter
        .iter()
        .zip(longer)
        .position(|(a, b)| a != b)
        .unwrap_or(shorter.len());
    if shorter.len() == longer.len() {
        mismatch < shorter.len() && shorter[mismatch + 1..] == longer[mismatch + 1..]
    } else {
        shorter[mismatch..] == longer[mismatch + 1..]
    }
}

fn snippet(text: &str, matched: Match) -> String {
    let (chars, offsets) = normalize(text);
    let start = matched.start.saturating_sub(36);
    let end = (matched.end + 56).max(start + 120).min(chars.len());
    let bytes_end = offsets.get(end).copied().unwrap_or(text.len());
    let body = text[offsets[start]..bytes_end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        body,
        if end < chars.len() { "…" } else { "" }
    )
}
