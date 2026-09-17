use clap::ValueEnum;
use serde::Serialize;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Rust,
    Typescript,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Category {
    pub id: &'static str,
    pub description: &'static str,
}

impl Language {
    pub fn for_path(path: &str) -> Option<Self> {
        match Path::new(path).extension()?.to_str()? {
            "rs" => Some(Self::Rust),
            "ts" | "tsx" | "mts" | "cts" => Some(Self::Typescript),
            _ => None,
        }
    }

    pub fn grammar(self, path: &str) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Typescript if path.ends_with(".tsx") => {
                tree_sitter_typescript::LANGUAGE_TSX.into()
            }
            Self::Typescript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        }
    }

    // Curated starting order, not a measured frequency ranking. Keep IDs stable.
    pub fn categories(self, top: usize) -> Vec<Category> {
        let entries = match self {
            Self::Rust => [
                (
                    "type_mismatch",
                    "Incompatible expression, argument, or return type",
                ),
                (
                    "name_resolution",
                    "Unresolved name, import, module, field, or method",
                ),
                (
                    "trait_bound",
                    "An unsatisfied trait bound or missing trait implementation",
                ),
                (
                    "moved_value",
                    "Use after move or moving out of a borrowed value",
                ),
                (
                    "borrow_conflict",
                    "Conflicting mutable or immutable borrows",
                ),
                (
                    "arguments",
                    "Wrong argument count or missing required struct fields",
                ),
                (
                    "lifetime",
                    "A borrowed value does not live long enough or lifetimes conflict",
                ),
                (
                    "mutability",
                    "Mutation through an immutable binding or reference",
                ),
                ("syntax", "Malformed Rust syntax"),
                (
                    "exhaustiveness",
                    "A non-exhaustive match or refutable binding pattern",
                ),
            ],
            Self::Typescript => [
                (
                    "type_mismatch",
                    "A value is not assignable to the required type",
                ),
                (
                    "missing_property",
                    "A required property is missing or accessed on a type without it",
                ),
                ("name_resolution", "An unresolved name, import, or module"),
                (
                    "nullability",
                    "Unsafe use of a possibly null or undefined value under the configured checks",
                ),
                (
                    "arguments",
                    "Wrong argument count or no compatible overload",
                ),
                (
                    "generic_constraint",
                    "An unsatisfied generic type constraint",
                ),
                (
                    "implicit_any",
                    "An implicit any forbidden by the configured compiler options",
                ),
                (
                    "return_type",
                    "A missing return or incompatible return contract",
                ),
                ("syntax", "Malformed TypeScript or TSX syntax"),
                (
                    "access",
                    "An invalid private/protected access or readonly assignment",
                ),
            ],
        };
        entries
            .into_iter()
            .take(top)
            .map(|(id, description)| Category { id, description })
            .collect()
    }
}

pub const OTHER: Category = Category {
    id: "other",
    description: "A compile-time error outside the explicitly selected categories",
};
