//! Reading a record's attributes in order, each by its type.

use crate::ImportError;
use crate::part21::Param;

/// An entity's instance number in the file.
pub type Id = u64;

/// The attributes of one record, read front to back.
pub(crate) struct Args<'a> {
    id: Id,
    params: &'a [Param],
    k: usize,
}

// Until the product tree (Task 11), some of these have no reader yet.
#[allow(dead_code)]
impl<'a> Args<'a> {
    /// The attributes of instance `id`; a record of any other length is
    /// `Malformed`.
    pub fn new(id: Id, params: &'a [Param], count: usize) -> Result<Self, ImportError> {
        if params.len() != count {
            return Err(ImportError::Malformed {
                instance: id,
                attribute: params.len().min(count),
                what: "wrong number of attributes",
            });
        }
        Ok(Args { id, params, k: 0 })
    }

    fn next(&mut self) -> &'a Param {
        let p = &self.params[self.k];
        self.k += 1;
        p
    }

    /// A fault in the attribute just read.
    pub fn fault(&self, what: &'static str) -> ImportError {
        ImportError::Malformed {
            instance: self.id,
            attribute: self.k.saturating_sub(1),
            what,
        }
    }

    /// Skips an attribute whose value nothing here needs (labels, derived
    /// values, descriptions).
    pub fn skip(&mut self) {
        self.next();
    }

    pub fn string(&mut self) -> Result<Box<str>, ImportError> {
        match self.next() {
            Param::Str(s) => Ok(s.clone()),
            Param::Unset => Ok("".into()),
            _ => Err(self.fault("expected a string")),
        }
    }

    pub fn real(&mut self) -> Result<f64, ImportError> {
        let p = self.next();
        real(p).ok_or_else(|| self.fault("expected a real"))
    }

    pub fn int(&mut self) -> Result<i64, ImportError> {
        match self.next() {
            Param::Int(n) => Ok(*n),
            _ => Err(self.fault("expected an integer")),
        }
    }

    pub fn id(&mut self) -> Result<Id, ImportError> {
        match self.next() {
            Param::Ref(r) => Ok(*r),
            _ => Err(self.fault("expected a reference")),
        }
    }

    /// A reference, or `$`.
    pub fn opt_id(&mut self) -> Result<Option<Id>, ImportError> {
        match self.next() {
            Param::Ref(r) => Ok(Some(*r)),
            Param::Unset => Ok(None),
            _ => Err(self.fault("expected a reference or $")),
        }
    }

    pub fn enumeration(&mut self) -> Result<&'a str, ImportError> {
        match self.next() {
            Param::Enum(e) => Ok(e),
            _ => Err(self.fault("expected an enumeration")),
        }
    }

    /// `.T.` or `.F.`.
    pub fn boolean(&mut self) -> Result<bool, ImportError> {
        match self.enumeration()? {
            "T" => Ok(true),
            "F" => Ok(false),
            _ => Err(self.fault("expected .T. or .F.")),
        }
    }

    /// `.T.`, `.F.` or `.U.` (unknown).
    pub fn logical(&mut self) -> Result<Option<bool>, ImportError> {
        match self.enumeration()? {
            "T" => Ok(Some(true)),
            "F" => Ok(Some(false)),
            "U" => Ok(None),
            _ => Err(self.fault("expected .T., .F. or .U.")),
        }
    }

    pub fn list(&mut self) -> Result<&'a [Param], ImportError> {
        match self.next() {
            Param::List(items) => Ok(items),
            _ => Err(self.fault("expected a list")),
        }
    }

    pub fn ids(&mut self) -> Result<Vec<Id>, ImportError> {
        let items = self.list()?;
        items
            .iter()
            .map(|p| match p {
                Param::Ref(r) => Ok(*r),
                _ => Err(self.fault("expected a list of references")),
            })
            .collect()
    }

    pub fn reals(&mut self) -> Result<Vec<f64>, ImportError> {
        let items = self.list()?;
        items
            .iter()
            .map(|p| real(p).ok_or_else(|| self.fault("expected a list of reals")))
            .collect()
    }
}

/// A real, also when written as an integer (exactly, up to 2^53) or as a
/// typed value such as `PARAMETER_VALUE(0.5)` or `LENGTH_MEASURE(25.4)`.
pub(crate) fn real(p: &Param) -> Option<f64> {
    match p {
        Param::Real(x) => Some(*x),
        Param::Int(n) if n.unsigned_abs() <= 1 << 53 => Some(*n as f64),
        Param::Typed(_, v) => real(v),
        _ => None,
    }
}
