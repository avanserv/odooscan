//! The placeholders of a message, as the formatting that fills it reads them.

/// What the `%` placeholders of a message take.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Specs {
    /// Arguments taken by position: one per placeholder without a key, and one per `*`.
    pub positional: usize,
    /// The keys of the `%(key)` placeholders, each once, in the order they first appear.
    pub keys: Vec<String>,
}

/// Conversions Python's `%` operator knows, `%` being the escaped percent sign.
const CONVERSIONS: &str = "diouxXeEfFgGcrsa%";

/// The printf-style placeholders of `text`, or the first one Python's `%` cannot read, from its `%`
/// to the character that breaks it or to the end of the text. A key may hold balanced parentheses,
/// and one length modifier among `hlL` is read and ignored, as Python does.
pub(crate) fn printf(text: &str) -> Result<Specs, String> {
    let mut specs = Specs::default();
    let mut rest = text;
    while let Some(at) = rest.find('%') {
        let spec = &rest[at..];
        let len = spec_len(spec, &mut specs).map_err(|end| spec[..end].to_owned())?;
        rest = &spec[len..];
    }
    Ok(specs)
}

/// The length of the placeholder `spec` starts with, adding what it takes to `specs`, or where the
/// part Python cannot read ends.
fn spec_len(spec: &str, specs: &mut Specs) -> Result<usize, usize> {
    let mut at = 1;
    let key = if spec[at..].starts_with('(') {
        Some(key(&spec[at..]).ok_or(spec.len())?)
    } else {
        None
    };
    at += key.map_or(0, |key| key.len() + 2);
    at += spec[at..].len() - spec[at..].trim_start_matches(['-', '+', ' ', '#', '0']).len();
    let stars = [width(spec, &mut at), precision(spec, &mut at)];
    if spec[at..].starts_with(['h', 'l', 'L']) {
        at += 1;
    }
    let conversion = spec[at..].chars().next().ok_or(spec.len())?;
    if !CONVERSIONS.contains(conversion) {
        return Err(at + conversion.len_utf8());
    }
    specs.positional += stars.iter().filter(|&&star| star).count();
    match key {
        Some(key) if !specs.keys.iter().any(|k| k == key) => specs.keys.push(key.to_owned()),
        None if conversion != '%' => specs.positional += 1,
        _ => {}
    }
    Ok(at + 1)
}

/// The key of a placeholder whose `%` precedes `after`, without its parentheses, which it may nest.
fn key(after: &str) -> Option<&str> {
    let inner = after.strip_prefix('(')?;
    let mut depth = 1;
    for (i, c) in inner.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 1 => return Some(&inner[..i]),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Moves `at` past a width, telling whether it is a `*` that takes an argument.
fn width(spec: &str, at: &mut usize) -> bool {
    let rest = &spec[*at..];
    if rest.starts_with('*') {
        *at += 1;
        return true;
    }
    *at += rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    false
}

/// Moves `at` past a precision, telling whether it is a `*` that takes an argument.
fn precision(spec: &str, at: &mut usize) -> bool {
    if !spec[*at..].starts_with('.') {
        return false;
    }
    *at += 1;
    width(spec, at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specs(positional: usize, keys: &[&str]) -> Result<Specs, String> {
        Ok(Specs {
            positional,
            keys: keys.iter().map(|&k| k.to_owned()).collect(),
        })
    }

    #[test]
    fn a_placeholder_without_a_key_takes_one_argument_by_position() {
        assert_eq!(printf("no placeholder"), specs(0, &[]));
        assert_eq!(printf("%s ordered %d"), specs(2, &[]));
        assert_eq!(printf("%r %a %c %i %o %u %x %X %e %E %f %F %g %G"), specs(14, &[]));
    }

    #[test]
    fn an_escaped_percent_takes_nothing() {
        assert_eq!(printf("100%% sure"), specs(0, &[]));
        assert_eq!(printf("%s at 100%%"), specs(1, &[]));
    }

    #[test]
    fn flags_width_precision_and_length_are_read() {
        assert_eq!(printf("%-5s %+d % d %#x %05.2f %.3s %ld %hd %Lf"), specs(9, &[]));
        assert_eq!(printf("%-#0 +12.4lf"), specs(1, &[]));
    }

    #[test]
    fn a_star_takes_one_argument_more() {
        assert_eq!(printf("%*d"), specs(2, &[]));
        assert_eq!(printf("%*.*f"), specs(3, &[]));
        assert_eq!(printf("%(total)*d"), specs(1, &["total"]));
    }

    #[test]
    fn keys_are_read_once_and_may_nest_parentheses() {
        assert_eq!(printf("%(name)s and %(count)d"), specs(0, &["name", "count"]));
        assert_eq!(printf("%(name)s, %(name)r"), specs(0, &["name"]));
        assert_eq!(printf("%(a(b))s"), specs(0, &["a(b)"]));
        assert_eq!(printf("%(name)s and %s"), specs(1, &["name"]));
        assert_eq!(printf("%(é)s"), specs(0, &["é"]));
    }

    #[test]
    fn the_first_broken_spec_is_returned() {
        assert_eq!(printf("Discount below 100 (got %)"), Err("%)".to_owned()));
        assert_eq!(printf("Order (%(order)) is locked"), Err("%(order))".to_owned()));
        assert_eq!(printf("%s and %y and %z"), Err("%y".to_owned()));
        assert_eq!(printf("%5.3hld"), Err("%5.3hl".to_owned()));
        assert_eq!(printf("%.é"), Err("%.é".to_owned()));
    }

    #[test]
    fn a_spec_the_text_ends_in_is_broken() {
        assert_eq!(printf("ends in %"), Err("%".to_owned()));
        assert_eq!(printf("ends in %-5."), Err("%-5.".to_owned()));
        assert_eq!(printf("%(unclosed s"), Err("%(unclosed s".to_owned()));
    }
}
