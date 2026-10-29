//! `misc/info`: the job's version, default units, name and source.

use super::error::{Code, Error, location};
use super::text::structured;
use super::units::Units;

/// What `misc/info` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    /// 7.0, 8.0 or 8.1.
    pub version: (u32, u32),
    /// The units of files without a `UNITS=` directive: inches unless
    /// `misc/info` says otherwise.
    pub units: Units,
    /// `PRODUCT_MODEL_NAME`, or `JOB_NAME` as older writers say.
    pub name: Option<String>,
    /// `SAVE_APP`, else `ODB_SOURCE`.
    pub application: Option<String>,
    pub created: Option<String>,
    pub saved: Option<String>,
    /// Version fields that were missing, read as 7.0.
    pub missing: Vec<&'static str>,
}

impl Info {
    /// A job without `misc/info`: 7.0 in inches.
    pub fn absent() -> Info {
        Info {
            version: (7, 0),
            units: Units::Inch,
            name: None,
            application: None,
            created: None,
            saved: None,
            missing: vec!["misc/info"],
        }
    }

    pub fn version_text(&self) -> String {
        format!("{}.{}", self.version.0, self.version.1)
    }
}

const PATH: &str = "misc/info";

/// Reads `misc/info`: a version other than 7.0, 8.0 or 8.1 is refused.
pub fn info(text: &str) -> Result<Info, Error> {
    let s = structured(text).map_err(|b| {
        Error::new(Code::Unreadable, b.message).at(location(PATH, Some(b.line), Some(b.column)))
    })?;
    let get = |k: &str| s.get(k).filter(|v| !v.is_empty()).map(str::to_string);
    let mut missing = Vec::new();
    let number =
        |k: &str| -> Result<Option<u32>, Error> {
            match s.field(k) {
                None => Ok(None),
                Some(f) => f.value.parse().map(Some).map_err(|_| {
                    Error::new(Code::Unreadable, format!("{k}={}: not a number", f.value))
                        .at(location(PATH, Some(f.line), None))
                }),
            }
        };
    let major = number("ODB_VERSION_MAJOR")?;
    let minor = number("ODB_VERSION_MINOR")?;
    let version = match (major, minor) {
        (Some(a), Some(b)) => (a, b),
        (Some(a), None) => {
            missing.push("ODB_VERSION_MINOR");
            (a, 0)
        }
        (None, _) => {
            missing.push("ODB_VERSION_MAJOR");
            (7, 0)
        }
    };
    if !matches!(version, (7, 0) | (8, 0) | (8, 1)) {
        return Err(Error::new(
            Code::VersionUnsupported,
            format!(
                "ODB++ {}.{}: only 7.0, 8.0 and 8.1 are read",
                version.0, version.1
            ),
        )
        .at(location(PATH, None, None)));
    }
    let units = match s.field("UNITS") {
        None => Units::Inch,
        Some(f) => Units::parse(&f.value).ok_or_else(|| {
            Error::new(
                Code::Unreadable,
                format!("UNITS={}: not MM or INCH", f.value),
            )
            .at(location(PATH, Some(f.line), None))
        })?,
    };
    Ok(Info {
        version,
        units,
        name: get("PRODUCT_MODEL_NAME").or_else(|| get("JOB_NAME")),
        application: get("SAVE_APP").or_else(|| get("ODB_SOURCE")),
        created: get("CREATION_DATE"),
        saved: get("SAVE_DATE"),
        missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_altium_and_kicad_write() {
        let altium = "UNITS=INCH\nPRODUCT_MODEL_NAME=odb\nODB_VERSION_MAJOR=8\nODB_VERSION_MINOR=1\nODB_SOURCE=Altium Designer\nCREATION_DATE=20260412.234630\nSAVE_DATE=20260412.234655\nSAVE_APP=Altium Designer Agile 26.5.0.17\n";
        let i = info(altium).unwrap();
        assert_eq!((i.version, i.units), ((8, 1), Units::Inch));
        assert_eq!(i.name.as_deref(), Some("odb"));
        assert_eq!(
            i.application.as_deref(),
            Some("Altium Designer Agile 26.5.0.17")
        );
        assert_eq!(i.created.as_deref(), Some("20260412.234630"));
        assert_eq!(i.saved.as_deref(), Some("20260412.234655"));
        assert!(i.missing.is_empty());
        let kicad = "JOB_NAME=job\nUNITS=MM\nODB_VERSION_MAJOR=8\nODB_VERSION_MINOR=1\nODB_SOURCE=KiCad EDA\nSAVE_APP=\n";
        let i = info(kicad).unwrap();
        assert_eq!((i.name.as_deref(), i.units), (Some("job"), Units::Mm));
        assert_eq!(i.application.as_deref(), Some("KiCad EDA"));
        assert_eq!(i.version_text(), "8.1");
    }

    #[test]
    fn versions() {
        assert_eq!(
            info("ODB_VERSION_MAJOR=8\nODB_VERSION_MINOR=0\n")
                .unwrap()
                .version,
            (8, 0)
        );
        let i = info("ODB_VERSION_MAJOR=7\n").unwrap();
        assert_eq!(
            (i.version, i.missing.clone()),
            ((7, 0), vec!["ODB_VERSION_MINOR"])
        );
        let i = info("UNITS=MM\n").unwrap();
        assert_eq!((i.version, i.units), ((7, 0), Units::Mm));
        for v in [
            "6\nODB_VERSION_MINOR=0",
            "8\nODB_VERSION_MINOR=2",
            "9\nODB_VERSION_MINOR=0",
        ] {
            let e = info(&format!("ODB_VERSION_MAJOR={v}\n")).unwrap_err();
            assert_eq!(e.code, Code::VersionUnsupported, "{v}");
        }
        assert_eq!(
            info("ODB_VERSION_MAJOR=x\n").unwrap_err().code,
            Code::Unreadable
        );
        assert_eq!(info("UNITS=CM\n").unwrap_err().code, Code::Unreadable);
        assert_eq!(info("A {\n").unwrap_err().code, Code::Unreadable);
        assert_eq!(Info::absent().version_text(), "7.0");
    }
}
