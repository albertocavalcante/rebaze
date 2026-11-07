//! Pure classification of conventional, top-level JUnit 4 source classes.
use crate::syntax::{Token, lex};
use anyhow::{Result, bail};

pub fn test_class(path: &str, source: &str) -> Result<Option<String>> {
    // Java processes Unicode escapes before lexing; this small lexer does not.
    if source.contains("\\u") || source.contains("\"\"\"") {
        bail!("unsupported Java escape/text block in {path}");
    }
    let tokens = lex(source)?;
    let words: Vec<_> = tokens
        .iter()
        .filter_map(|token| match token {
            Token::Word(word) => Some(word.as_str()),
            _ => None,
        })
        .collect();
    if words.contains(&"extends") {
        bail!("test-source inheritance requires Java/Gradle evaluation: {path}");
    }
    if words.windows(3).any(|w| w == ["org", "junit", "jupiter"])
        || words.contains(&"TestCase")
        || words.contains(&"RunWith")
        || words.contains(&"Ignore")
    {
        bail!("only direct, non-ignored JUnit 4 @Test classes are supported: {path}");
    }
    let annotations = tokens
        .windows(2)
        .any(|w| w == [Token::Symbol('@'), Token::Word("Test".into())])
        || tokens.windows(6).any(|w| {
            w == [
                Token::Symbol('@'),
                Token::Word("org".into()),
                Token::Symbol('.'),
                Token::Word("junit".into()),
                Token::Symbol('.'),
                Token::Word("Test".into()),
            ]
        });
    if !annotations {
        return Ok(None);
    }
    if words.contains(&"abstract") {
        bail!("abstract JUnit classes require Java/Gradle evaluation: {path}");
    }
    if !words.windows(3).any(|w| w == ["org", "junit", "Test"])
        && !tokens.windows(6).any(|w| {
            w == [
                Token::Word("import".into()),
                Token::Word("org".into()),
                Token::Symbol('.'),
                Token::Word("junit".into()),
                Token::Symbol('.'),
                Token::Symbol('*'),
            ]
        })
    {
        bail!("@Test must refer to org.junit.Test: {path}");
    }
    let classes: Vec<_> = tokens
        .windows(2)
        .filter_map(|w| match (&w[0], &w[1]) {
            (Token::Word(keyword), Token::Word(name)) if keyword == "class" => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let class_index = tokens
        .iter()
        .position(|token| token == &Token::Word("class".into()));
    let public_class =
        class_index.is_some_and(|index| tokens[..index].contains(&Token::Word("public".into())));
    if classes.len() != 1 || !public_class {
        bail!("tests require one public top-level class: {path}");
    }
    let expected = path
        .strip_prefix("src/test/java/")
        .and_then(|p| p.strip_suffix(".java"))
        .ok_or_else(|| anyhow::anyhow!("nonconventional test path {path}"))?
        .replace('/', ".");
    if expected.rsplit('.').next() != Some(classes[0]) {
        bail!("test class/file mismatch: {path}");
    }
    let mut package = String::new();
    if let Some(index) = tokens
        .iter()
        .position(|t| t == &Token::Word("package".into()))
    {
        for token in &tokens[index + 1..] {
            match token {
                Token::Word(word) => package.push_str(word),
                Token::Symbol('.') => package.push('.'),
                Token::Symbol(';') => break,
                _ => bail!("unsupported package declaration in {path}"),
            }
        }
    }
    let actual = if package.is_empty() {
        classes[0].into()
    } else {
        format!("{package}.{}", classes[0])
    };
    if actual != expected {
        bail!("test package/path mismatch: {path}");
    }
    Ok(Some(actual))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn test_discovery_uses_annotations_and_ignores_comments_and_strings() {
        assert_eq!(test_class("src/test/java/example/Behavior.java", "package example; import org.junit.Test; public class Behavior { @Test public void value() {} }").unwrap().as_deref(),Some("example.Behavior"));
        assert_eq!(
            test_class(
                "src/test/java/Helper.java",
                "// @Test class Fake {}\npublic class Helper { String value = \"@Test\"; }"
            )
            .unwrap(),
            None
        );
        assert!(
            test_class(
                "src/test/java/Wrong.java",
                "import org.junit.Test; public class Other { @Test public void x() {} }"
            )
            .is_err()
        );
    }

    #[test]
    fn test_inheritance_and_abstract_classes_require_evaluation() {
        for source in [
            "public class Child extends Base {}",
            "import org.junit.Test; public abstract class Child { @Test public void value() {} }",
            "import org.junit.Test; class Child { @Test public void value() {} }",
        ] {
            assert!(
                test_class("src/test/java/Child.java", source).is_err(),
                "{source}"
            );
        }
    }
}
