//! The stackup: copper and dielectric layers in stack order, with their
//! materials, thicknesses and properties.
//!
//! From `matrix/stackup.xml` where the exporter writes one (its
//! `EdaData` stackup: each layer's material by its spec reference, the
//! material's default thickness in the file's units, a dielectric's Dk
//! and Df, a conductor's copper weight), else from the matrix's copper
//! and dielectric board layers in row order: a dielectric's thickness is
//! its `.layer_dielectric` attribute, in its attribute list's units; a
//! copper layer's is its `.copper_weight`, which is microns in a metric
//! list and ounces per square foot in an imperial one (1 oz/ft² is 1.37
//! mils of copper, IPC's figure). Each keeps its `.dielectric_constant`,
//! `.loss_tangent` and `.copper_weight` as properties.

use std::collections::BTreeMap;

use ryu_board::geom::Nm;
use ryu_board::geom::decimal::{self, Unit};
use ryu_board::model::{AttrValue, Attributes, Context, LayerKind, StackKind, StackLayer, Stackup};

use super::matrix::MatrixLayer;
use super::units::Units;

pub const PATH: &str = "matrix/stackup.xml";

/// Nanometres of copper in one ounce per square foot: 1.37 mils.
const OUNCE: i128 = 34_798;

fn unit(s: &str) -> Option<Unit> {
    match s.to_ascii_uppercase().as_str() {
        "MIL" => Some(Unit::Mil),
        "INCH" => Some(Unit::Inch),
        "MICRON" => Some(Unit::Micron),
        "MM" => Some(Unit::Mm),
        _ => None,
    }
}

fn number(v: &str) -> Option<AttrValue> {
    decimal::scaled(v, 1, i64::MAX)
        .is_ok()
        .then(|| AttrValue::Number(v.to_string()))
}

/// The child elements of `n` named `tag`, in any case.
fn children<'a, 'i>(
    n: roxmltree::Node<'a, 'i>,
    tag: &'static str,
) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> {
    n.children()
        .filter(move |c| c.is_element() && c.tag_name().name().eq_ignore_ascii_case(tag))
}

/// Reads `matrix/stackup.xml`'s design stackup.
pub fn from_xml(text: &str) -> Result<Stackup, String> {
    let doc = roxmltree::Document::parse(text).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let default = root
        .attribute("DefaultUnits")
        .map_or(Some(Unit::Mil), unit)
        .ok_or("DefaultUnits: not MIL, INCH, MICRON or MM")?;
    let eda = children(root, "EdaData").next().ok_or("no EdaData")?;
    // Materials by spec and name.
    let mut materials: BTreeMap<(String, String), roxmltree::Node> = BTreeMap::new();
    for specs in children(eda, "Specs") {
        for spec in children(specs, "Spec") {
            let s = spec.attribute("SpecName").unwrap_or("").to_string();
            for m in children(spec, "Material") {
                let name = m.attribute("MaterialName").unwrap_or("").to_string();
                materials.insert((s.clone(), name), m);
            }
        }
    }
    let stack = children(eda, "Stackup").next().ok_or("no Stackup")?;
    let mut out = Vec::new();
    for group in children(stack, "Group") {
        for layer in children(group, "Layer") {
            let name = layer
                .attribute("LayerName")
                .ok_or("a Layer without LayerName")?;
            let kind = match layer
                .attribute("LayerType")
                .unwrap_or("DOCUMENT")
                .to_ascii_uppercase()
                .as_str()
            {
                "SIGNAL" | "POWER_GROUND" | "MIXED" => StackKind::Copper,
                "DIELECTRIC" => StackKind::Dielectric,
                _ => continue,
            };
            let mut l = StackLayer {
                name: name.to_ascii_lowercase(),
                kind,
                material: None,
                thickness: None,
                properties: Attributes::new(),
            };
            for r in children(layer, "SpecRef") {
                let spec = r.attribute("MaterialSpecName").unwrap_or("").to_string();
                for m in children(r, "Material") {
                    let Some(mname) = m.attribute("MaterialName") else {
                        continue;
                    };
                    l.material = Some(mname.to_string());
                    let Some(def) = materials.get(&(spec.clone(), mname.to_string())) else {
                        return Err(format!("{name}: no material {mname} in spec {spec}"));
                    };
                    if let Some(t) = children(*def, "Default_Thickness").next() {
                        let u = t
                            .attribute("Units")
                            .map_or(Some(default), unit)
                            .ok_or("Units: not MIL, INCH, MICRON or MM")?;
                        if let Some(v) = t.attribute("Thickness") {
                            l.thickness = Some(
                                decimal::nm(v, u)
                                    .map_err(|e| format!("{name}: Thickness={v}: {e}"))?
                                    .value,
                            );
                        }
                    }
                    for d in children(*def, "Dielectric") {
                        for props in children(d, "Properties") {
                            for p in children(props, "Property") {
                                for (attr, key) in [
                                    ("DielectricConstant_Dk", "dielectric_constant"),
                                    ("LossTangent_Df", "loss_tangent"),
                                ] {
                                    if let Some(v) = p.attribute(attr).and_then(number) {
                                        l.properties.entry(key.to_string()).or_insert(v);
                                    }
                                }
                            }
                        }
                    }
                    for c in children(*def, "Conductor") {
                        if let Some(v) = c.attribute("CopperWeight_oz_ft2").and_then(number) {
                            l.properties.insert("copper_weight_oz_ft2".into(), v);
                        }
                    }
                }
            }
            out.push(l);
        }
    }
    Ok(Stackup { layers: out })
}

/// The stackup the matrix and each board layer's attributes (with the
/// units of its attribute list) give, if the matrix has a dielectric
/// layer.
pub fn from_matrix(layers: &[MatrixLayer], attrs: &[(Attributes, Units)]) -> Option<Stackup> {
    let mut out = Vec::new();
    for (l, (a, units)) in layers.iter().zip(attrs) {
        if l.context != Context::Board {
            continue;
        }
        let kind = match l.kind {
            LayerKind::Dielectric => StackKind::Dielectric,
            ref k if k.is_copper() => StackKind::Copper,
            _ => continue,
        };
        let thickness: Option<Nm> =
            match (kind, a.get(".layer_dielectric"), a.get(".copper_weight")) {
                (StackKind::Dielectric, Some(AttrValue::Number(v)), _) => {
                    decimal::nm(v, units.coord()).ok().map(|e| e.value)
                }
                (StackKind::Copper, _, Some(AttrValue::Number(w))) => match units {
                    Units::Mm => decimal::nm(w, Unit::Micron).ok().map(|e| e.value),
                    // 1.37 mil is 34,798 nm.
                    Units::Inch => decimal::scaled(w, OUNCE, 1 << 40).ok().map(|e| e.value),
                },
                _ => None,
            };
        let mut properties = Attributes::new();
        for (attr, key) in [
            (".dielectric_constant", "dielectric_constant"),
            (".loss_tangent", "loss_tangent"),
            (".copper_weight", "copper_weight"),
        ] {
            if let Some(v) = a.get(attr) {
                properties.insert(key.to_string(), v.clone());
            }
        }
        out.push(StackLayer {
            name: l.name.clone(),
            kind,
            material: l.dielectric_name.clone(),
            thickness,
            properties,
        });
    }
    out.iter()
        .any(|l| l.kind == StackKind::Dielectric)
        .then_some(Stackup { layers: out })
}

/// Whether every layer of a stackup has a thickness: the `stackup`
/// capability.
pub fn complete(s: &Stackup) -> bool {
    !s.layers.is_empty() && s.layers.iter().all(|l| l.thickness.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::odb::matrix::matrix;

    const XML: &str = r#"<?xml version="1.0"?>
<StackupFile Version="8.1" DefaultUnits="MICRON">
 <EdaData CompanyName="Valor">
  <Specs>
   <Spec SpecName="Valor">
    <Material MaterialName="0.5 oz CU">
     <Conductor ConductorType="COPPER" CopperWeight_oz_ft2="0.5"/>
     <Default_Thickness Thickness="17.5"/>
    </Material>
    <Material MaterialName="Core">
     <Dielectric DielectricType="CORE">
      <Properties PropertyName="p">
       <Property FrequencyVal="1" DielectricConstant_Dk="4.3" LossTangent_Df="0.02"/>
      </Properties>
     </Dielectric>
     <Default_Thickness Thickness="59" Units="MIL"/>
    </Material>
   </Spec>
  </Specs>
  <Stackup StackupName="pcb" StackupThickness="85">
   <Group GroupName="Design">
    <Layer LayerName="TOP" LayerType="SIGNAL" Side="TOP"><SpecRef MaterialSpecName="Valor"><Material MaterialName="0.5 oz CU"/></SpecRef></Layer>
    <Layer LayerName="SM" LayerType="SOLDER_MASK" Side="TOP"/>
    <Layer LayerName="core" LayerType="DIELECTRIC" Side="INNER"><SpecRef MaterialSpecName="Valor"><Material MaterialName="Core"/></SpecRef></Layer>
    <Layer LayerName="bot" LayerType="SIGNAL" Side="BOTTOM"/>
   </Group>
  </Stackup>
 </EdaData>
</StackupFile>"#;

    #[test]
    fn the_stackup_file() {
        let s = from_xml(XML).unwrap();
        let got: Vec<_> = s
            .layers
            .iter()
            .map(|l| (l.name.as_str(), l.kind, l.material.as_deref(), l.thickness))
            .collect();
        assert_eq!(
            got,
            [
                ("top", StackKind::Copper, Some("0.5 oz CU"), Some(17_500)),
                ("core", StackKind::Dielectric, Some("Core"), Some(1_498_600)),
                ("bot", StackKind::Copper, None, None),
            ]
        );
        assert_eq!(
            s.layers[0].properties.get("copper_weight_oz_ft2"),
            Some(&AttrValue::Number("0.5".into()))
        );
        assert_eq!(
            s.layers[1].properties.get("dielectric_constant"),
            Some(&AttrValue::Number("4.3".into()))
        );
        assert_eq!(
            s.layers[1].properties.get("loss_tangent"),
            Some(&AttrValue::Number("0.02".into()))
        );
        assert!(!complete(&s));
        assert!(from_xml("<x").is_err());
        assert_eq!(from_xml("<StackupFile/>").unwrap_err(), "no EdaData");
        assert!(
            from_xml(&XML.replace("MaterialName=\"Core\"/>", "MaterialName=\"Gone\"/>"))
                .unwrap_err()
                .contains("no material Gone")
        );
    }

    #[test]
    fn the_matrix_and_attributes() {
        let m = matrix("STEP {\nCOL=1\nNAME=pcb\n}\nLAYER {\nROW=1\nCONTEXT=BOARD\nTYPE=SOLDER_MASK\nNAME=sm\n}\nLAYER {\nROW=2\nCONTEXT=BOARD\nTYPE=SIGNAL\nNAME=top\n}\nLAYER {\nROW=3\nCONTEXT=BOARD\nTYPE=DIELECTRIC\nNAME=d1\nDIELECTRIC_NAME=FR-4\n}\nLAYER {\nROW=4\nCONTEXT=BOARD\nTYPE=SIGNAL\nNAME=bot\n}\nLAYER {\nROW=5\nCONTEXT=MISC\nTYPE=DIELECTRIC\nNAME=x\n}\n").unwrap();
        let a = |pairs: &[(&str, &str)]| -> Attributes {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), AttrValue::Number(v.to_string())))
                .collect()
        };
        // Altium writes .layer_dielectric on copper too, as the core's.
        let attrs = vec![
            (a(&[(".layer_dielectric", "0.0005")]), Units::Inch),
            (
                a(&[(".layer_dielectric", "0.059"), (".copper_weight", "0.5")]),
                Units::Inch,
            ),
            (
                a(&[
                    (".layer_dielectric", "1.5"),
                    (".dielectric_constant", "4.3"),
                ]),
                Units::Mm,
            ),
            (a(&[(".copper_weight", "35")]), Units::Mm),
            (Attributes::new(), Units::Inch),
        ];
        let s = from_matrix(&m.layers, &attrs).unwrap();
        let got: Vec<_> = s
            .layers
            .iter()
            .map(|l| (l.name.as_str(), l.kind, l.thickness))
            .collect();
        assert_eq!(
            got,
            [
                ("top", StackKind::Copper, Some(17_399)),
                ("d1", StackKind::Dielectric, Some(1_500_000)),
                ("bot", StackKind::Copper, Some(35_000)),
            ]
        );
        assert_eq!(s.layers[1].material.as_deref(), Some("FR-4"));
        assert_eq!(
            s.layers[0].properties.get("copper_weight"),
            Some(&AttrValue::Number("0.5".into()))
        );
        assert!(complete(&s));
        // Two copper layers and no dielectric: no stackup.
        assert!(from_matrix(&m.layers[..2], &attrs[..2]).is_none());
    }
}
