//! Simple `printf`-like template formatting.

/// Template for formatting names using a provided pattern.
///
/// Supported template formats are:
/// * `%d` for simple number substitution e.g. `out%d.ppm` -> `out9.ppm`, `out10.ppm`
/// * `%Nd` for space-padded number substitution e.g. `out%4d.ppm` -> `out  99.ppm`
/// * `%0Nd` for zero-padded number substitution e.g. `out%04d.ppm` -> `out0099.ppm`
/// * and of course `%%` can be used to output single percent sign.
pub struct TemplateName {
    prefix:         String,
    pad_size:       usize,
    suffix:         String,
    single:         bool,
}

trait Deescape {
    fn deescape(&mut self);
}

impl Deescape for String {
    fn deescape(&mut self) {
        while let Some(idx) = self.find("%%") {
            self.remove(idx + 1);
        }
    }
}

impl TemplateName {
    /// Creates a new instance of `TemplateName` from the provided pattern.
    pub fn new(name: &str) -> Self {
        let mut off = 0;
        let mut tmpl_start = 0;
        let mut tmpl_end = 0;
        let mut pad_size = 0;
        'parse_loop:
        while let Some(idx) = name[off..].find('%') {
            let idx = idx + off;
            if idx + 1 == name.len() {
                break;
            }
            if name[idx + 1..].starts_with('%') { // escape, skip it
                off += 1;
            }
            if name[idx + 1..].starts_with('0') {
                if let Some(end_idx) = name[idx + 2..].find('d') {
                    if let Ok(val) = name[idx + 2..][..end_idx].parse::<usize>() {
                        if val <= 32 {
                            tmpl_start = idx;
                            pad_size = val;
                            tmpl_end = idx + 2 + end_idx + 1;
                        }
                    }
                    break 'parse_loop;
                }
            }
            if name[idx + 1..].starts_with('d') {
                tmpl_start = idx;
                tmpl_end = idx + 2;
                break;
            }
            off += idx;
        }

        if tmpl_end == 0 {
            let mut prefix = name.to_owned();
            prefix.deescape();
            Self {
                prefix,
                pad_size:   0,
                suffix:     String::new(),
                single:     true,
            }
        } else {
            let mut prefix = name[..tmpl_start].to_string();
            prefix.deescape();
            let mut suffix = name[tmpl_end..].to_string();
            suffix.deescape();
            Self {
                prefix, suffix, pad_size,
                single: false,
            }
        }
    }
    /// Reports whether template has no substitutions e.g. "output" vs. "output%d"
    pub fn is_single(&self) -> bool { self.single }
    /// Returns a string formatted using initial pattern and provided argument.
    ///
    /// If the initial pattern had no formatting specifier, this pattern will be returned instead.
    pub fn format<T: Sized+ToString>(&self, id: T) -> String {
        let mut number = id.to_string();
        while number.len() < self.pad_size {
            number.insert(0, '0');
        }
        let mut fname = String::with_capacity(self.prefix.len() + number.len() + self.suffix.len());
        fname.push_str(&self.prefix);
        fname.push_str(&number);
        fname.push_str(&self.suffix);
        fname
    }
}
