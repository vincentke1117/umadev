/// One character's relationship to the quote state shared by every Git-commit
/// parser. Keeping one lexer prevents the safety scanner, literal tokenizer,
/// and control-text projection from disagreeing about escaped quotes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QuoteEvent {
    /// A normal character outside quotes.
    Outside,
    /// The opening delimiter of a quoted region.
    Opened,
    /// A normal character inside quotes.
    Inside,
    /// A backslash introducing one escaped character inside an ASCII quote.
    EscapePrefix,
    /// The character immediately following an escape prefix.
    Escaped,
    /// A backslash followed by a non-escapable character; both are literal.
    LiteralEscape,
    /// The closing delimiter of a quoted region.
    Closed,
}

/// Small quote-state machine used by all host Git parsers.
///
/// ASCII single/double quotes accept backslash escapes because literal commit
/// messages arrive as shell-like text. Curly/CJK quotes are delimiter-only.
/// Backticks are optional: natural-language scope/control projections treat
/// them as quoting punctuation, while literal commands leave them outside so
/// the unsafe-connector firewall rejects command substitution.
#[derive(Debug, Clone)]
pub(super) struct QuoteTracker {
    closing: Option<char>,
    escaped: bool,
    previous: Option<char>,
    quote_backticks: bool,
}

impl QuoteTracker {
    pub(super) const fn new(quote_backticks: bool) -> Self {
        Self {
            closing: None,
            escaped: false,
            previous: None,
            quote_backticks,
        }
    }

    pub(super) fn step(&mut self, character: char) -> QuoteEvent {
        let event = if let Some(closing) = self.closing {
            if self.escaped {
                self.escaped = false;
                if matches!(character, '\\') || character == closing {
                    QuoteEvent::Escaped
                } else {
                    QuoteEvent::LiteralEscape
                }
            } else if character == '\\' && matches!(closing, '\'' | '"') {
                self.escaped = true;
                QuoteEvent::EscapePrefix
            } else if character == closing {
                self.closing = None;
                QuoteEvent::Closed
            } else {
                QuoteEvent::Inside
            }
        } else if let Some(closing) = self.opening_quote(character) {
            self.closing = Some(closing);
            QuoteEvent::Opened
        } else {
            QuoteEvent::Outside
        };
        self.previous = Some(character);
        event
    }

    pub(super) const fn is_balanced(&self) -> bool {
        self.closing.is_none() && !self.escaped
    }

    fn opening_quote(&self, character: char) -> Option<char> {
        match character {
            '"' => Some('"'),
            '\'' if !self.previous.is_some_and(char::is_alphanumeric) => Some('\''),
            '`' if self.quote_backticks => Some('`'),
            '“' => Some('”'),
            '‘' => Some('’'),
            '「' => Some('」'),
            '『' => Some('』'),
            _ => None,
        }
    }
}
