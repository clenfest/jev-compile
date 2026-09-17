use super::*;
use std::collections::BTreeMap;

impl Snapshot {
    pub fn state(&self, questions: &BTreeMap<usize, Vec<Span>>) -> Value {
        let windows: BTreeMap<_, Vec<_>> = questions
            .iter()
            .map(|(index, regions)| {
                let windows = screen_spans(&self.files[*index])
                    .into_iter()
                    .filter(|window| {
                        regions.iter().any(|region| {
                            window.start_line <= region.end_line
                                && region.start_line <= window.end_line
                        })
                    })
                    .collect();
                (*index, windows)
            })
            .collect();
        let names: BTreeSet<_> = windows
            .iter()
            .flat_map(|(index, windows)| {
                self.files[*index]
                    .syntax
                    .identifier_spans
                    .iter()
                    .filter_map(|(name, spans)| {
                        spans
                            .iter()
                            .any(|span| {
                                windows.iter().any(|window| {
                                    window.start_line <= span.end_line
                                        && span.start_line <= window.end_line
                                })
                            })
                            .then_some(name)
                    })
            })
            .collect();
        let mut index: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for (file_index, file) in self.files.iter().enumerate() {
            for (name, entries) in &file.syntax.declarations {
                if names.contains(name) {
                    for entry in entries {
                        index.entry(name).or_default().push((file_index, entry));
                    }
                }
            }
        }
        let mut declarations = Vec::new();
        let mut related: BTreeSet<_> = windows.keys().copied().collect();
        let mut ambiguous = BTreeSet::new();
        let mut support_bytes = 0;
        let mut omitted_count = 0;
        let mut omitted_examples = Vec::new();
        for (name, entries) in index {
            for (file_index, entry) in &entries {
                if windows.get(file_index).is_some_and(|windows| {
                    windows.iter().any(|window| {
                        window.start_line <= entry.span.start_line
                            && entry.span.end_line <= window.end_line
                    })
                }) {
                    continue;
                }
                let file = &self.files[*file_index];
                // Common method names are not a repository-wide dependency graph.
                // Keep unique declarations or same-directory candidates, and expose
                // ambiguity instead of pretending this is compiler name resolution.
                let nearby = windows.keys().any(|target| {
                    Path::new(&self.files[*target].path).parent() == Path::new(&file.path).parent()
                });
                if entries.len() > 1 && !nearby {
                    ambiguous.insert(name);
                    continue;
                }
                let declaration = json!({
                    "file": file.path, "symbol": name, "revision": file.revision,
                    "start_line": entry.span.start_line, "source": entry.evidence,
                });
                let bytes = declaration.to_string().len();
                if support_bytes + bytes > 16_000 {
                    omitted_count += 1;
                    if omitted_examples.len() < 10 {
                        omitted_examples.push(json!({"file": file.path, "symbol": name}));
                    }
                    continue;
                }
                support_bytes += bytes;
                related.insert(*file_index);
                declarations.push(declaration);
            }
        }
        let paths: BTreeSet<_> = related
            .iter()
            .map(|index| &self.files[*index].path)
            .collect();
        json!({
            "base_commit": self.base_commit,
            "snapshot": self.fingerprint,
            "language": self.language,
            "changed_files": self.changes.iter().map(|change| &change.path).collect::<Vec<_>>(),
            "changes": self.changes.iter().filter(|change| paths.contains(&change.path)).collect::<Vec<_>>(),
            "files": windows.iter().map(|(index, windows)| {
                let file = &self.files[*index];
                json!({"path": file.path, "revision": file.revision,
                    "source_regions": windows,
                    "source": windows.iter().map(|region| syntax::numbered_region(&file.source, *region)).collect::<Vec<_>>().join("\n[intervening lines omitted; see source_regions]\n")})
            }).collect::<Vec<_>>(),
            "context": self.context,
            "related_declarations": declarations,
            "omitted_support": {"declarations": omitted_count, "examples": omitted_examples,
                "reason": "Supporting declarations have a 16000-byte budget; full candidate windows are never truncated. Excluded declarations are not evidence that a name is absent."},
            "ambiguous_symbols_outside_target_directories": ambiguous,
            "context_strategy": "Exhaustive candidate windows are screened separately; this batch includes its complete windows and relevant file diffs. Small files fit in one window. Rust function bodies are omitted from supporting declarations. Ambiguous names outside candidate directories are excluded. This is lexical matching, not compiler name resolution; do not infer that excluded declarations are absent. Ask for insufficient context when needed.",
            "warnings": self.warnings,
        })
    }
}
