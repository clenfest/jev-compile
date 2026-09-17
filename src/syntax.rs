use crate::{language::Language, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::{Node, Parser};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Span {
    pub start_line: usize,
    pub end_line: usize,
}

impl Span {
    pub fn whole(source: &str) -> Self {
        Self {
            start_line: 1,
            end_line: source.lines().count().max(1),
        }
    }

    pub fn width(self) -> usize {
        self.end_line - self.start_line + 1
    }
}

pub struct Syntax {
    pub declarations: BTreeMap<String, Vec<Span>>,
    pub identifiers: BTreeSet<String>,
    boundaries: BTreeSet<usize>,
}

impl Syntax {
    pub fn parse(language: Language, path: &str, source: &str) -> Result<Self> {
        let mut parser = Parser::new();
        parser.set_language(&language.grammar(path))?;
        let tree = parser.parse(source, None).ok_or("could not parse source")?;
        let mut syntax = Self {
            declarations: BTreeMap::new(),
            identifiers: BTreeSet::new(),
            boundaries: BTreeSet::new(),
        };
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            let kind = node.kind();
            if kind.contains("identifier") {
                syntax
                    .identifiers
                    .insert(node.utf8_text(source.as_bytes())?.to_owned());
            }
            let declaration = matches!(
                kind,
                "function_item"
                    | "struct_item"
                    | "enum_item"
                    | "trait_item"
                    | "type_item"
                    | "const_item"
                    | "static_item"
                    | "function_declaration"
                    | "method_definition"
                    | "interface_declaration"
                    | "type_alias_declaration"
                    | "class_declaration"
                    | "variable_declarator"
            );
            if declaration {
                if let Some(name) = node.child_by_field_name("name") {
                    syntax
                        .declarations
                        .entry(name.utf8_text(source.as_bytes())?.to_owned())
                        .or_default()
                        .push(declaration_span(node));
                }
            }
            if declaration
                || kind.ends_with("statement")
                || kind.ends_with("declaration")
                || kind.ends_with("expression")
            {
                let range = span(node);
                syntax.boundaries.insert(range.start_line);
                syntax.boundaries.insert(range.end_line + 1);
            }
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
        }
        Ok(syntax)
    }

    /// Disjoint, exhaustive line regions. Prefer syntax boundaries near balanced
    /// cuts; malformed source still makes progress using line boundaries.
    pub fn partition(&self, region: Span) -> Vec<Span> {
        if region.width() <= 8 {
            return (region.start_line..=region.end_line)
                .map(|line| Span {
                    start_line: line,
                    end_line: line,
                })
                .collect();
        }
        let mut cuts = BTreeSet::from([region.start_line, region.end_line + 1]);
        for part in 1..4 {
            let target = region.start_line + region.width() * part / 4;
            let cut = self
                .boundaries
                .range((region.start_line + 1)..=region.end_line)
                .min_by_key(|line| line.abs_diff(target))
                .copied()
                .filter(|line| line.abs_diff(target) <= region.width() / 8)
                .unwrap_or(target);
            cuts.insert(cut);
        }
        let cuts: Vec<_> = cuts.into_iter().collect();
        cuts.windows(2)
            .map(|pair| Span {
                start_line: pair[0],
                end_line: pair[1] - 1,
            })
            .collect()
    }
}

fn span(node: Node<'_>) -> Span {
    let end = node.end_position();
    Span {
        start_line: node.start_position().row + 1,
        end_line: (end.row + usize::from(end.column > 0)).max(node.start_position().row + 1),
    }
}

fn declaration_span(node: Node<'_>) -> Span {
    let mut parent = node.parent();
    while let Some(enclosing) = parent {
        if matches!(
            enclosing.kind(),
            "impl_item" | "trait_item" | "class_declaration" | "interface_declaration"
        ) {
            return span(enclosing);
        }
        parent = enclosing.parent();
    }
    span(node)
}

pub fn numbered(source: &str) -> String {
    source
        .lines()
        .enumerate()
        .map(|(index, line)| format!("{}: {line}\n", index + 1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partitions_preserve_every_line_and_make_progress_even_on_broken_code() {
        for (language, path, source) in [
            (
                Language::Rust,
                "x.rs",
                "fn outer() {\n let a = 1;\n let b = 2;\n}\n".repeat(12),
            ),
            (
                Language::Typescript,
                "x.tsx",
                "export function A() {\n return <p>{broken(}</p>;\n}\n".repeat(12),
            ),
        ] {
            let syntax = Syntax::parse(language, path, &source).unwrap();
            let mut queue = vec![Span::whole(&source)];
            let mut lines = Vec::new();
            while let Some(region) = queue.pop() {
                if region.width() == 1 {
                    lines.push(region.start_line);
                    continue;
                }
                let children = syntax.partition(region);
                assert!(children.iter().all(|child| child.width() < region.width()));
                queue.extend(children);
            }
            lines.sort_unstable();
            assert_eq!(lines, (1..=source.lines().count()).collect::<Vec<_>>());
        }
    }
}
