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

/// What the `{}` fields of a message take, as `str.format()` reads them.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Fields {
    /// Fields numbered automatically: one per `{}`, or per `{!r}` or `{:>5}` without a name.
    pub auto: usize,
    /// The names and numbers fields take an argument by, each once, in the order they first appear.
    pub names: Vec<String>,
    /// Every field is named by an identifier, a number or nothing, with no attribute or index after
    /// it, so the text reads as a message meant for `str.format()`.
    pub simple: bool,
}

/// The `{}` fields of `text`, or the first part `str.format()` refuses: a lone `}`, a field never
/// closed, an unknown conversion, a field nested twice, or the switch between automatic and manual
/// numbering, which it refuses whatever the arguments.
pub(crate) fn braces(text: &str) -> Result<Fields, String> {
    let mut fields = Fields {
        simple: true,
        ..Fields::default()
    };
    let mut numbered = false;
    let mut rest = text;
    while let Some(at) = rest.find(['{', '}']) {
        let tail = &rest[at..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            rest = &tail[2..];
            continue;
        }
        let len = tail
            .strip_prefix('{')
            .and_then(closing)
            .ok_or_else(|| field_text(tail))?;
        let field = &tail[..len + 2];
        add_field(&field[1..=len], 1, &mut fields, &mut numbered).map_err(|()| field.to_owned())?;
        rest = &tail[len + 2..];
    }
    Ok(fields)
}

/// What an error reports of the field `tail` opens: a lone `}`, or all that follows a `{`.
fn field_text(tail: &str) -> String {
    if tail.starts_with('}') { "}" } else { tail }.to_owned()
}

/// The length of the body of a field whose `{` precedes `inner`, up to its matching `}`.
fn closing(inner: &str) -> Option<usize> {
    let mut depth = 1;
    for (i, c) in inner.char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 1 => return Some(i),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Adds what the field `body` takes to `fields`. A field in a format spec is at `depth` 2, and
/// Python refuses one deeper still.
fn add_field(body: &str, depth: usize, fields: &mut Fields, numbered: &mut bool) -> Result<(), ()> {
    let name_len = field_name_len(body);
    let (name, after) = body.split_at(name_len);
    let spec = match after.strip_prefix('!') {
        Some(conversion) => conversion_spec(conversion)?,
        None => after.strip_prefix(':').unwrap_or_default(),
    };
    let arg = &name[..name.find(['.', '[']).unwrap_or(name.len())];
    fields.simple &= arg.len() == name.len() && (arg.is_empty() || is_number(arg) || is_identifier(arg));
    if arg.is_empty() {
        fields.auto += 1;
    } else if !fields.names.iter().any(|n| n == arg) {
        fields.names.push(arg.to_owned());
    }
    *numbered |= is_number(arg);
    if fields.auto > 0 && *numbered {
        return Err(());
    }
    nested_fields(spec, depth, fields, numbered)
}

/// The length of the name a field `body` opens with, up to a `!` or `:` outside an index.
fn field_name_len(body: &str) -> usize {
    let mut in_index = false;
    for (i, c) in body.char_indices() {
        match c {
            '[' => in_index = true,
            ']' => in_index = false,
            '!' | ':' if !in_index => return i,
            _ => {}
        }
    }
    body.len()
}

/// The format spec after a conversion, which is one of `r`, `s` and `a`, alone or before `:`.
fn conversion_spec(conversion: &str) -> Result<&str, ()> {
    let mut chars = conversion.chars();
    let known = chars.next().is_some_and(|c| "rsa".contains(c));
    let rest = chars.as_str();
    match rest.strip_prefix(':') {
        Some(spec) if known => Ok(spec),
        None if known && rest.is_empty() => Ok(""),
        _ => Err(()),
    }
}

/// Adds the fields a format spec holds, such as the `{width}` of `{total:>{width}}`.
fn nested_fields(spec: &str, depth: usize, fields: &mut Fields, numbered: &mut bool) -> Result<(), ()> {
    let mut rest = spec;
    while let Some(at) = rest.find('{') {
        if depth > 1 {
            return Err(());
        }
        let len = closing(&rest[at + 1..]).ok_or(())?;
        add_field(&rest[at + 1..at + 1 + len], depth + 1, fields, numbered)?;
        rest = &rest[at + len + 2..];
    }
    Ok(())
}

pub(crate) fn is_number(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit())
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_alphabetic()) && chars.all(|c| c == '_' || c.is_alphanumeric())
}

/// The placeholders of `text` as the web client's `sprintf()` fills them: `%s` by position and
/// `%(key)s` by key, `%%` being an escaped percent sign. Any other `%` is text.
pub(crate) fn sprintf(text: &str) -> Specs {
    let mut specs = Specs::default();
    let mut rest = text;
    while let Some(at) = rest.find('%') {
        rest = &rest[at + 1..];
        if let Some(after) = rest.strip_prefix(['%', 's']) {
            specs.positional += usize::from(rest.starts_with('s'));
            rest = after;
        } else if let Some((key, after)) = sprintf_key(rest) {
            if !specs.keys.iter().any(|k| k == key) {
                specs.keys.push(key.to_owned());
            }
            rest = after;
        }
    }
    specs
}

/// The key of a `%(key)s` whose `%` precedes `after`, and the text after it.
fn sprintf_key(after: &str) -> Option<(&str, &str)> {
    let (key, rest) = after.strip_prefix('(')?.split_once(')')?;
    (!key.is_empty()).then_some((key, rest.strip_prefix('s')?))
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

    fn fields(auto: usize, names: &[&str], simple: bool) -> Result<Fields, String> {
        Ok(Fields {
            auto,
            names: names.iter().map(|&n| n.to_owned()).collect(),
            simple,
        })
    }

    #[test]
    fn a_field_takes_an_argument_by_name_number_or_position() {
        assert_eq!(braces("no field"), fields(0, &[], true));
        assert_eq!(
            braces("{name} ordered {count} and {name}"),
            fields(0, &["name", "count"], true)
        );
        assert_eq!(braces("{} of {}"), fields(2, &[], true));
        assert_eq!(braces("{1} before {0}"), fields(0, &["1", "0"], true));
        assert_eq!(braces("{total!r:>10} {:.2f} {é}"), fields(1, &["total", "é"], true));
    }

    #[test]
    fn doubled_braces_are_text() {
        assert_eq!(braces("{{ not a field }} {name}"), fields(0, &["name"], true));
    }

    #[test]
    fn an_attribute_an_index_or_another_name_is_not_simple() {
        assert_eq!(braces("{order.name}"), fields(0, &["order"], false));
        assert_eq!(braces("{lines[0]}"), fields(0, &["lines"], false));
        assert_eq!(braces("{ name }"), fields(0, &[" name "], false));
        assert_eq!(braces(r#"{"key": 1}"#), fields(0, &[r#""key""#], false));
    }

    #[test]
    fn a_format_spec_may_hold_one_level_of_fields() {
        assert_eq!(braces("{total:>{width}}"), fields(0, &["total", "width"], true));
        assert_eq!(braces("{a:{b:{c}}}"), Err("{a:{b:{c}}}".to_owned()));
    }

    #[test]
    fn what_format_refuses_is_returned() {
        assert_eq!(braces("total } here"), Err("}".to_owned()));
        assert_eq!(braces("{name} and {open"), Err("{open".to_owned()));
        assert_eq!(braces("{name!x}"), Err("{name!x}".to_owned()));
        assert_eq!(braces("{name!rs}"), Err("{name!rs}".to_owned()));
        assert_eq!(braces("{} and {0}"), Err("{0}".to_owned()));
        assert_eq!(braces("{0} and {}"), Err("{}".to_owned()));
    }

    #[test]
    fn sprintf_reads_s_and_keys_and_skips_the_rest() {
        assert_eq!(sprintf("%s of %s"), specs(2, &[]).unwrap());
        assert_eq!(
            sprintf("%(name)s and %(count)s, %(name)s"),
            specs(0, &["name", "count"]).unwrap()
        );
        assert_eq!(sprintf("100%% of %s"), specs(1, &[]).unwrap());
        assert_eq!(sprintf("%%s is text, as are %d, % and %()s"), specs(0, &[]).unwrap());
        assert_eq!(sprintf("%(open s"), specs(0, &[]).unwrap());
    }
}
