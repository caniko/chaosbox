use chaosbox_core::SourceSpan;
use scip::types::{occurrence::Typed_range, Occurrence};
use super::{invalid, CompilerError, Encoding};

pub(super) struct Positions<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> Positions<'a> {
    pub fn new(text: &'a str) -> Result<Self, CompilerError> {
        if text.len() > u32::MAX as usize {
            return Err(invalid("source exceeds span byte limit"));
        }
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Ok(Self { text, starts })
    }

    fn coordinate(
        &self,
        line: i32,
        column: i32,
        encoding: Encoding,
    ) -> Result<(u32, u32), CompilerError> {
        let line = usize::try_from(line).map_err(|_| invalid("negative line"))?;
        let column = usize::try_from(column).map_err(|_| invalid("negative column"))?;
        let start = *self
            .starts
            .get(line)
            .ok_or_else(|| invalid("line outside document"))?;
        let end = self.starts.get(line + 1).map_or(self.text.len(), |v| v - 1);
        let text = self.text[start..end]
            .strip_suffix('\r')
            .unwrap_or(&self.text[start..end]);
        let mut units = 0;
        let mut scalars = 0;
        let mut byte = 0;
        for ch in text.chars() {
            if units >= column {
                break;
            }
            units += match encoding {
                Encoding::Utf8 => ch.len_utf8(),
                Encoding::Utf16 => ch.len_utf16(),
                Encoding::Utf32 => 1,
            };
            byte += ch.len_utf8();
            scalars += 1;
        }
        if units != column {
            return Err(invalid("column outside line or splitting a code point"));
        }
        Ok((
            u32::try_from(start + byte).map_err(|_| invalid("byte overflow"))?,
            scalars + 1,
        ))
    }

    pub fn span(
        &self,
        file: &str,
        occurrence: &Occurrence,
        encoding: Encoding,
    ) -> Result<SourceSpan, CompilerError> {
        let legacy = match occurrence.range.as_slice() {
            [] => None,
            [sl, sc, ec] => Some([*sl, *sc, *sl, *ec]),
            [sl, sc, el, ec] => Some([*sl, *sc, *el, *ec]),
            _ => return Err(invalid("legacy range must have three or four coordinates")),
        };
        let typed = match &occurrence.typed_range {
            Some(Typed_range::SingleLineRange(r)) => {
                Some([r.line, r.start_character, r.line, r.end_character])
            }
            Some(Typed_range::MultiLineRange(r)) => {
                Some([r.start_line, r.start_character, r.end_line, r.end_character])
            }
            None => None,
            Some(_) => return Err(invalid("unsupported typed range")),
        };
        if typed.zip(legacy).is_some_and(|(a, b)| a != b) {
            return Err(invalid("typed and legacy ranges disagree"));
        }
        let [sl, sc, el, ec] = typed
            .or(legacy)
            .ok_or_else(|| invalid("missing occurrence range"))?;
        let (byte_start, start_col) = self.coordinate(sl, sc, encoding)?;
        let (byte_end, end_col) = self.coordinate(el, ec, encoding)?;
        if byte_end < byte_start {
            return Err(invalid("reversed range"));
        }
        Ok(SourceSpan {
            file: file.into(),
            start_line: u32::try_from(sl).map_err(|_| invalid("negative line"))? + 1,
            start_col,
            end_line: u32::try_from(el).map_err(|_| invalid("negative line"))? + 1,
            end_col,
            byte_start,
            byte_end,
        })
    }
}
