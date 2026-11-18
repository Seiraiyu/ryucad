//! The CSV reader (design 734): RFC 4180 records in any one-character
//! delimiter, each with the line it starts on.

use super::rows::{Raw, Refusal};

/// The delimiter the first line most likely uses: the most frequent of
/// `,` `;` tab `|` outside quotes, `,` on a tie or none.
pub fn sniff(text: &str) -> char {
    let first = text.lines().next().unwrap_or("");
    let mut quoted = false;
    let mut counts = [0usize; 4];
    for c in first.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => counts[0] += 1,
            ';' if !quoted => counts[1] += 1,
            '\t' if !quoted => counts[2] += 1,
            '|' if !quoted => counts[3] += 1,
            _ => {}
        }
    }
    let best = (0..4)
        .max_by_key(|&i| (counts[i], std::cmp::Reverse(i)))
        .unwrap_or(0);
    if counts[best] == 0 {
        ','
    } else {
        [',', ';', '\t', '|'][best]
    }
}

pub fn parse(text: &str, delimiter: char) -> Result<Vec<Raw>, Refusal> {
    if delimiter == '"' || delimiter == '\n' || delimiter == '\r' {
        return Err(Refusal::file(
            "a delimiter can't be a quote or a line break",
        ));
    }
    let mut out: Vec<Raw> = Vec::new();
    let mut line = 1u64;
    let mut start = 1u64;
    let mut cells: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut opened = 0u64;
    let mut chars = text.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        any = true;
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    cell.push('"');
                }
                '"' => quoted = false,
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    line += 1;
                    cell.push('\n');
                }
                '\n' => {
                    line += 1;
                    cell.push('\n');
                }
                c => cell.push(c),
            }
            continue;
        }
        match c {
            '"' if cell.is_empty() => {
                quoted = true;
                opened = line;
            }
            c if c == delimiter => cells.push(std::mem::take(&mut cell)),
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                cells.push(std::mem::take(&mut cell));
                out.push((start, std::mem::take(&mut cells)));
                line += 1;
                start = line;
                any = false;
                if out.len() > super::MAX_ROWS {
                    return Err(Refusal::file("more than 2,000,000 rows"));
                }
            }
            c => cell.push(c),
        }
    }
    if quoted {
        return Err(Refusal::at(
            opened,
            None,
            "a quote opened on this line is never closed",
        ));
    }
    if any || !cell.is_empty() || !cells.is_empty() {
        cells.push(cell);
        out.push((start, cells));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(r: &Raw) -> Vec<&str> {
        r.1.iter().map(String::as_str).collect()
    }

    #[test]
    fn quoted_fields_hold_delimiters_quotes_and_lines() {
        let rows = parse(
            "a,b,c\r\n\"x, y\",\"say \"\"hi\"\"\",\"two\nlines\"\n1,,3",
            ',',
        )
        .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(cells(&rows[0]), ["a", "b", "c"]);
        assert_eq!(cells(&rows[1]), ["x, y", "say \"hi\"", "two\nlines"]);
        assert_eq!(rows[1].0, 2);
        assert_eq!(rows[2].0, 4, "the third record starts on line 4");
        assert_eq!(cells(&rows[2]), ["1", "", "3"]);
    }

    #[test]
    fn any_delimiter_and_a_last_line_without_a_break() {
        let rows = parse("a;b\n1;2", ';').unwrap();
        assert_eq!(cells(&rows[1]), ["1", "2"]);
        assert_eq!(parse("a\tb\n", '\t').unwrap().len(), 1);
        assert_eq!(sniff("\"x;y\",a,b\n"), ',');
        assert_eq!(sniff("Id;Designator;Qty\n"), ';');
        assert_eq!(sniff("a\tb\tc\n"), '\t');
        assert_eq!(sniff("one column\n"), ',');
    }

    #[test]
    fn an_open_quote_is_refused_where_it_opened() {
        let e = parse("a\nb,\"never\nclosed", ',').unwrap_err();
        assert_eq!(e.row, 2);
        assert!(parse("a", '"').is_err());
    }
}
