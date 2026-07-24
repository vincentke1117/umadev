use super::quote::{QuoteEvent, QuoteTracker};
use super::LiteralGitCommitSpec;

pub(super) fn literal_git_commit_tail(command: &str) -> Option<&str> {
    let prefix = command.get(.."git commit".len())?;
    if !prefix.eq_ignore_ascii_case("git commit") {
        return None;
    }
    let tail = &command["git commit".len()..];
    tail.chars()
        .next()
        .is_none_or(|character| character.is_whitespace() || character == '-')
        .then_some(tail)
}

pub(super) fn literal_git_commit_policy(command: &str) -> Result<LiteralGitCommitSpec, ()> {
    let Some(tail) = literal_git_commit_tail(command) else {
        return Err(());
    };
    let Some(tokens) = tokenize_literal_git_commit_tail(tail) else {
        return Err(());
    };
    let mut message = None;
    let mut cursor = 0;
    while cursor < tokens.len() {
        let token = &tokens[cursor];
        if matches!(token.as_str(), "and" | "then")
            || token.starts_with("然后")
            || token.starts_with("然後")
            || token.starts_with("并")
            || token.starts_with("並")
        {
            break;
        }
        match token.as_str() {
            "-m" | "--message" => {
                cursor += 1;
                let value = tokens
                    .get(cursor)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or(())?;
                message = Some(value.clone());
            }
            value if value.starts_with("--message=") && value.len() > "--message=".len() => {
                message = Some(value["--message=".len()..].to_string());
            }
            _ => return Err(()),
        }
        cursor += 1;
    }
    Ok(LiteralGitCommitSpec { message })
}

fn tokenize_literal_git_commit_tail(tail: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quotes = QuoteTracker::new(false);
    for character in tail.chars() {
        match quotes.step(character) {
            QuoteEvent::Opened | QuoteEvent::Closed | QuoteEvent::EscapePrefix => {}
            QuoteEvent::Inside | QuoteEvent::Escaped => current.push(character),
            QuoteEvent::LiteralEscape => {
                current.push('\\');
                current.push(character);
            }
            QuoteEvent::Outside
                if character.is_whitespace() || matches!(character, ',' | '，' | '；') =>
            {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            QuoteEvent::Outside if matches!(character, ';' | '|' | '&' | '>' | '<' | '`') => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                break;
            }
            QuoteEvent::Outside => current.push(character),
        }
    }
    if !quotes.is_balanced() {
        return None;
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Some(tokens)
}
