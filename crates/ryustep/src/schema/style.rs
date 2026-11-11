//! Presentation (ISO 10303-46): styled items and the surface styles a
//! face can take, as CAx-IF's *Model Styling and Organization* gives
//! them, read by [`crate::styles`] (design 448, plan 24.B). The usage
//! paths an over-riding style's context names are assembly structure:
//! `SPECIFIED_HIGHER_USAGE_OCCURRENCE` is typed here beside them.

use super::Entity;
use super::args::{Args, Id};
use crate::ImportError;
use crate::part21::Param;

/// The side of a surface a `SURFACE_STYLE_USAGE` styles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Both,
    Positive,
    Negative,
}

/// A simple record of a presentation entity, or `None`.
pub(super) fn simple(id: Id, name: &str, params: &[Param]) -> Option<Result<Entity, ImportError>> {
    Some(match name {
        // STYLED_ITEM(name, styles, item)
        "STYLED_ITEM" => styled(id, params, 3),
        // OVER_RIDING_STYLED_ITEM(name, styles, item, over_ridden_style)
        "OVER_RIDING_STYLED_ITEM" => styled(id, params, 4),
        // CONTEXT_DEPENDENT_OVER_RIDING_STYLED_ITEM(name, styles, item,
        // over_ridden_style, style_context)
        "CONTEXT_DEPENDENT_OVER_RIDING_STYLED_ITEM" => styled(id, params, 5),
        // PRESENTATION_STYLE_ASSIGNMENT(styles)
        "PRESENTATION_STYLE_ASSIGNMENT" => (|| {
            let mut a = Args::new(id, params, 1)?;
            Ok(Entity::StyleAssignment { styles: a.refs()? })
        })(),
        // SURFACE_STYLE_USAGE(side, style)
        "SURFACE_STYLE_USAGE" => (|| {
            let mut a = Args::new(id, params, 2)?;
            let side = match a.enumeration()? {
                "BOTH" => Side::Both,
                "POSITIVE" => Side::Positive,
                "NEGATIVE" => Side::Negative,
                _ => return Err(a.fault("expected .BOTH., .POSITIVE. or .NEGATIVE.")),
            };
            Ok(Entity::SurfaceStyleUsage {
                side,
                style: a.id()?,
            })
        })(),
        // SURFACE_SIDE_STYLE(name, styles)
        "SURFACE_SIDE_STYLE" => (|| {
            let mut a = Args::new(id, params, 2)?;
            a.skip();
            Ok(Entity::SurfaceSideStyle { styles: a.refs()? })
        })(),
        // SURFACE_STYLE_FILL_AREA(fill_area)
        "SURFACE_STYLE_FILL_AREA" => (|| {
            let mut a = Args::new(id, params, 1)?;
            Ok(Entity::SurfaceFill { fill: a.id()? })
        })(),
        // FILL_AREA_STYLE(name, fill_styles)
        "FILL_AREA_STYLE" => (|| {
            let mut a = Args::new(id, params, 2)?;
            a.skip();
            Ok(Entity::FillAreaStyle { styles: a.refs()? })
        })(),
        // FILL_AREA_STYLE_COLOUR(name, fill_colour)
        "FILL_AREA_STYLE_COLOUR" => (|| {
            let mut a = Args::new(id, params, 2)?;
            a.skip();
            Ok(Entity::FillColour { colour: a.id()? })
        })(),
        // COLOUR_RGB(name, red, green, blue)
        "COLOUR_RGB" => (|| {
            let mut a = Args::new(id, params, 4)?;
            a.skip();
            let r = a.real()?;
            let g = a.real()?;
            let b = a.real()?;
            Ok(Entity::ColourRgb { rgb: [r, g, b] })
        })(),
        // DRAUGHTING_PRE_DEFINED_COLOUR(name)
        "DRAUGHTING_PRE_DEFINED_COLOUR" => (|| {
            let mut a = Args::new(id, params, 1)?;
            Ok(Entity::PredefinedColour { name: a.string()? })
        })(),
        // SURFACE_STYLE_RENDERING_WITH_PROPERTIES(rendering_method,
        // surface_colour, properties): read for its transparency alone.
        "SURFACE_STYLE_RENDERING_WITH_PROPERTIES" => (|| {
            let mut a = Args::new(id, params, 3)?;
            a.skip();
            a.skip();
            Ok(Entity::Rendering {
                properties: a.refs()?,
            })
        })(),
        // SURFACE_STYLE_TRANSPARENT(transparency)
        "SURFACE_STYLE_TRANSPARENT" => (|| {
            let mut a = Args::new(id, params, 1)?;
            Ok(Entity::Transparent {
                transparency: a.real()?,
            })
        })(),
        // SPECIFIED_HIGHER_USAGE_OCCURRENCE(id, name, description,
        // relating_product_definition, related_product_definition,
        // reference_designator, upper_usage, next_usage)
        "SPECIFIED_HIGHER_USAGE_OCCURRENCE" => (|| {
            let mut a = Args::new(id, params, 8)?;
            for _ in 0..6 {
                a.skip();
            }
            Ok(Entity::HigherUsage {
                upper: a.id()?,
                next: a.id()?,
            })
        })(),
        _ => return None,
    })
}

fn styled(id: Id, params: &[Param], count: usize) -> Result<Entity, ImportError> {
    let mut a = Args::new(id, params, count)?;
    a.skip();
    let styles = a.refs()?;
    let item = a.id()?;
    if count >= 4 {
        // The style it overrides: what a reader shows elsewhere.
        a.skip();
    }
    let context = if count == 5 { Some(a.refs()?) } else { None };
    Ok(Entity::StyledItem {
        styles,
        item,
        over_riding: count >= 4,
        context,
    })
}
