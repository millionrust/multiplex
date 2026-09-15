use std::io::{self, BufRead};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BoundedLine {
    Bytes(Vec<u8>),
    TooLong,
}

pub(crate) fn read_bounded_lines(
    mut reader: impl BufRead,
    max_line_bytes: usize,
    mut on_line: impl FnMut(BoundedLine),
) -> io::Result<()> {
    let mut line = Vec::new();
    let mut too_long = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if too_long {
                on_line(BoundedLine::TooLong);
            } else if !line.is_empty() {
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                on_line(BoundedLine::Bytes(line));
            }
            return Ok(());
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let content_len = newline.unwrap_or(available.len());
        if !too_long {
            if line.len().saturating_add(content_len) > max_line_bytes {
                line.clear();
                too_long = true;
            } else {
                line.extend_from_slice(&available[..content_len]);
            }
        }
        let consumed = newline.map_or(available.len(), |index| index + 1);
        reader.consume(consumed);

        if newline.is_some() {
            if too_long {
                on_line(BoundedLine::TooLong);
            } else {
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                on_line(BoundedLine::Bytes(std::mem::take(&mut line)));
            }
            line.clear();
            too_long = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BoundedLine, read_bounded_lines};
    use std::io::{BufReader, Cursor};

    #[test]
    fn preserves_crlf_empty_and_final_unterminated_lines() {
        let reader = BufReader::with_capacity(3, Cursor::new(b"one\r\n\ntwo"));
        let mut lines = Vec::new();
        read_bounded_lines(reader, 8, |line| lines.push(line)).unwrap();
        assert_eq!(
            lines,
            vec![
                BoundedLine::Bytes(b"one".to_vec()),
                BoundedLine::Bytes(Vec::new()),
                BoundedLine::Bytes(b"two".to_vec()),
            ]
        );
    }

    #[test]
    fn discards_an_oversized_line_and_resumes_at_the_next_record() {
        let reader = BufReader::with_capacity(2, Cursor::new(b"12345\nok\nabcdef"));
        let mut lines = Vec::new();
        read_bounded_lines(reader, 4, |line| lines.push(line)).unwrap();
        assert_eq!(
            lines,
            vec![
                BoundedLine::TooLong,
                BoundedLine::Bytes(b"ok".to_vec()),
                BoundedLine::TooLong,
            ]
        );
    }
}
