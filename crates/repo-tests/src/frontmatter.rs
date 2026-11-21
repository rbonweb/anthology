//! Markdown frontmatter parsing (MT1): `load_frontmatter` of `tests/_repo.py`.

use crate::yaml::{self, Yaml};

/// Splits `text` into its frontmatter and its body, as `_repo.load_frontmatter` does: the text
/// starts with `---\n`, the frontmatter runs to the first `\n---`, and the body loses its leading
/// newlines. An empty frontmatter is an empty mapping. `name` only labels the errors.
pub fn parse(name: &str, text: &str) -> Result<(Yaml, String), String> {
    let rest = text
        .strip_prefix("---\n")
        .ok_or_else(|| format!("{name}: missing frontmatter"))?;
    let (head, body) = rest
        .split_once("\n---")
        .ok_or_else(|| format!("{name}: unterminated frontmatter"))?;
    let mut data = yaml::parse(head).map_err(|e| format!("{name}: {e}"))?;
    if data.is_null() {
        data = Yaml::Hash(Default::default());
    }
    Ok((data, body.trim_start_matches('\n').to_string()))
}

/// `parse` of the repo-relative markdown file `rel`; panics on any error.
pub fn load(rel: &str) -> (Yaml, String) {
    parse(rel, &crate::repo::read(rel)).unwrap_or_else(|e| panic!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_head_and_body() {
        let (meta, body) = parse("x", "---\nname: a\n---\n\n\nbody\n").unwrap();
        assert_eq!(meta["name"].as_str(), Some("a"));
        assert_eq!(body, "body\n");
    }

    #[test]
    fn errors_name_the_file() {
        assert_eq!(parse("x", "no").unwrap_err(), "x: missing frontmatter");
        assert_eq!(
            parse("x", "---\na: 1\n").unwrap_err(),
            "x: unterminated frontmatter"
        );
        let (meta, _) = parse("x", "---\n\n---\nb").unwrap();
        assert!(meta.as_hash().unwrap().is_empty());
    }
}
