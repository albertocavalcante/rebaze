//! Small lexical boundary shared by the literal Gradle subset and Java headers.

use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Word(String),
    Literal(String),
    Symbol(char),
}

pub fn lex(source: &str) -> Result<Vec<Token>> {
    if source.len() > 1024 * 1024 {
        bail!(
            "Gradle/Java source byte budget exceeded: limit 1048576, observed {}",
            source.len()
        );
    }
    let mut chars = source.chars().peekable();
    let mut tokens = Vec::new();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {}
            '/' if chars.peek() == Some(&'/') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut closed = false;
                while let Some(next) = chars.next() {
                    if next == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    bail!("unterminated block comment");
                }
            }
            '\'' | '"' => {
                let mut value = String::new();
                let mut closed = false;
                while let Some(next) = chars.next() {
                    if next == c {
                        closed = true;
                        break;
                    }
                    value.push(next);
                    if next == '\\' {
                        value.push(
                            chars
                                .next()
                                .ok_or_else(|| anyhow::anyhow!("unterminated escape"))?,
                        );
                    }
                }
                if !closed {
                    bail!("unterminated string");
                }
                tokens.push(Token::Literal(value));
            }
            c if c.is_ascii_alphanumeric() || matches!(c, '_' | '$') => {
                let mut word = String::from(c);
                while chars
                    .peek()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$'))
                {
                    if let Some(next) = chars.next() {
                        word.push(next);
                    }
                }
                tokens.push(Token::Word(word));
            }
            c => tokens.push(Token::Symbol(c)),
        }
    }
    Ok(tokens)
}

pub struct Cursor {
    tokens: Vec<Token>,
    position: usize,
}

impl Cursor {
    pub fn new(source: &str) -> Result<Self> {
        Ok(Self {
            tokens: lex(source)?,
            position: 0,
        })
    }
    pub const fn done(&self) -> bool {
        self.position == self.tokens.len()
    }
    pub fn word_is(&mut self, name: &str) -> bool {
        if self.tokens.get(self.position) == Some(&Token::Word(name.into())) {
            self.position += 1;
            true
        } else {
            false
        }
    }
    pub fn take(&mut self) -> Result<Token> {
        let token = self
            .tokens
            .get(self.position)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unexpected end of Gradle source"))?;
        self.position += 1;
        Ok(token)
    }
    pub fn symbol(&mut self, symbol: char) -> bool {
        if self.tokens.get(self.position) == Some(&Token::Symbol(symbol)) {
            self.position += 1;
            true
        } else {
            false
        }
    }
    pub fn expect(&mut self, symbol: char) -> Result<()> {
        if !self.symbol(symbol) {
            bail!("expected {symbol:?} in literal Gradle subset");
        }
        Ok(())
    }
    pub fn word(&mut self) -> Result<String> {
        match self.take()? {
            Token::Word(word) => Ok(word),
            other => bail!("expected a Gradle name, found {other:?}"),
        }
    }
    pub fn literal(&mut self) -> Result<String> {
        match self.take()? {
            Token::Literal(value)
                if !value.contains(['$', '\\']) && !value.chars().any(char::is_control) =>
            {
                Ok(value)
            }
            other => bail!("expected an uninterpolated, unescaped Gradle literal, found {other:?}"),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod limit_tests {
    use super::*;

    #[test]
    fn borrowed_source_byte_limits_count_utf8_before_token_allocation() {
        let source = format!("//{}", "é".repeat((1024 * 1024 - 2) / 2));
        assert_eq!(source.len(), 1024 * 1024);
        assert_eq!(lex(&source).unwrap(), [] as [Token; 0]);
        let error = lex(&(source + "x")).unwrap_err();
        assert!(error.to_string().contains("source byte budget"));
        assert!(error.to_string().len() < 256);
    }
}
