//! The router: one `match` over every request, each arm a call of its
//! handler (ryucad 27.B).

use super::cache::{cache_stats, cache_warm};
use super::feature::{
    Given, feature_add, feature_delete, feature_edit, feature_list, feature_move,
};
use super::part::{check_format, part_create, part_delete, part_list, project_info};
use super::{annotation, assembly, cache, feature, git, pdm, session, view};
use super::{noted, versioned};
use crate::error::{Code, Error, Result};
use crate::live::Live;
use crate::proto::Request;
use crate::session::Session;
use serde_json::{Value, json};

pub(crate) fn dispatch(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    req: Request,
) -> Result<Value> {
    match req {
        Request::ProjectInit { .. } => Err(Error::new(
            Code::ProjectExists,
            format!("{} already holds a project", session.root().display()),
        )
        .with("path", session.root())),
        Request::ProjectInfo {} => Ok(project_info(session)),
        Request::PartCreate { part, kind } => part_create(session, &part, kind.as_deref()),
        Request::PartDelete {
            part,
            expected_revision,
        } => part_delete(session, &part, expected_revision.as_deref()),
        Request::PartList {} => Ok(part_list(session)),
        Request::FeatureAdd {
            part,
            ty,
            params,
            placement,
            fields,
            id,
            position,
            expected_revision,
            dry_run,
        } => feature_add(
            session,
            &part,
            &ty,
            Given {
                params,
                placement,
                fields,
            },
            id.as_deref(),
            &position,
            expected_revision.as_deref(),
            dry_run,
        ),
        Request::FeatureEdit {
            part,
            id,
            params,
            placement,
            fields,
            expressions,
            expected_revision,
            dry_run,
        } => feature_edit(
            session,
            &part,
            &id,
            Given {
                params,
                placement,
                fields,
            },
            &expressions,
            expected_revision.as_deref(),
            dry_run,
        ),
        Request::FeatureDelete {
            part,
            id,
            expected_revision,
        } => feature_delete(session, &part, &id, expected_revision.as_deref()),
        Request::FeatureMove {
            part,
            id,
            position,
            expected_revision,
        } => feature_move(session, &part, &id, &position, expected_revision.as_deref()),
        Request::FeatureList { part } => feature_list(session, &part),
        Request::PdmInit {} => crate::pdm::init(session),
        Request::PdmNewId { expected_revision } => {
            crate::pdm::new_id(session, expected_revision.as_deref())
        }
        Request::PdmConfig {} => crate::pdm::config(session),
        Request::PdmConfigure {
            revisions,
            release_branch,
            pattern,
            properties,
            approvals,
            bump,
            package_formats,
            force,
            expected_revision,
        } => crate::pdm::configure(
            session,
            crate::pdm::Change {
                revisions,
                release_branch,
                pattern,
                properties,
                approvals,
                bump,
                package_formats,
                force,
            },
            expected_revision.as_deref(),
        ),
        Request::PropsGet { part, rev } => pdm::props_get(session, part, rev),
        Request::BomGet {
            part,
            rev,
            flat,
            depth,
            format,
        } => crate::bom::get(
            session,
            &crate::bom::Ask {
                part: part.as_deref(),
                rev: rev.as_deref(),
                flat,
                depth,
                format: format.as_deref(),
            },
        ),
        Request::BomDiff { part, from, to } => {
            crate::bom::diff(session, part.as_deref(), from.as_deref(), to.as_deref())
        }
        Request::BomWhereUsed {
            part,
            label,
            working,
            direct,
            within,
            no_dependents,
        } => crate::bom::where_used(
            session,
            &crate::bom::WhereUsed {
                part: &part,
                label: label.as_deref(),
                working,
                direct,
                within: &within,
                no_dependents,
            },
        ),
        Request::BomRenumber {
            part,
            items,
            compact,
            expected_revision,
        } => crate::bom::renumber(
            session,
            &part,
            &items,
            compact,
            expected_revision.as_deref(),
        ),
        Request::ReleaseList { part } => versioned(crate::release::list(session, part.as_deref())),
        Request::ReleaseSync {} => crate::release::sync(session),
        Request::ReleaseCheck { results } => crate::release::check(session, results),
        Request::ReleasePackageVerify { file, rebuild } => {
            crate::package::verify(session, &file, rebuild)
        }
        Request::ReleasePropose {
            part,
            note,
            expected_revision,
            dry_run,
        } => pdm::release_propose(session, live, me, part, note, expected_revision, dry_run),
        Request::ReleaseObsolete {
            part,
            label,
            note,
            expected_revision,
        } => pdm::release_obsolete(session, live, me, part, label, note, expected_revision),
        Request::PropsSet {
            part,
            properties,
            expected_revision,
        } => crate::pdm::set(session, &part, properties, expected_revision.as_deref()),
        Request::ParamList { part, scope, rev } => feature::param_list(session, part, scope, rev),
        Request::ParamSet {
            part,
            scope,
            parameters,
            expected_revision,
        } => feature::param_set(session, part, scope, parameters, expected_revision),
        Request::ParamDelete {
            part,
            scope,
            name,
            expected_revision,
        } => feature::param_delete(session, part, scope, name, expected_revision),
        Request::ParamRename {
            part,
            scope,
            from,
            to,
            expected_revision,
        } => feature::param_rename(session, part, scope, from, to, expected_revision),
        Request::SketchEdit {
            part,
            sketch,
            ops,
            expected_revision,
        } => crate::sketch::edit(session, &part, &sketch, &ops, expected_revision.as_deref()),
        Request::SketchSolve { part, sketch, ops } => {
            crate::sketch::solve_request(session, &part, &sketch, &ops)
        }
        Request::SketchGet {
            part,
            sketch,
            quality,
            rev,
        } => feature::sketch_get(session, part, sketch, quality, rev),
        Request::PathGet {
            part,
            path,
            quality,
            rev,
        } => feature::path_get(session, part, path, quality, rev),
        Request::SweepGet {
            part,
            feature,
            quality,
            rev,
        } => feature::sweep_get(session, part, feature, quality, rev),
        Request::AssemblyMove {
            part,
            instance,
            placement,
            translate,
            axis,
            angle,
            about,
            expected_revision,
        } => crate::assembly::moved(
            session,
            &part,
            &instance,
            crate::assembly::Move {
                placement,
                translate,
                axis,
                angle,
                about,
            },
            expected_revision.as_deref(),
        ),
        Request::AssemblyTree { part, path, depth } => {
            crate::assembly::tree(session, &part, path.as_deref(), depth)
        }
        Request::HoleDerive { spec, fields, part } => {
            crate::hole::derive::request(session, spec.as_deref(), fields.as_ref(), part.as_deref())
        }
        Request::Interference { part, instances } => {
            crate::interference::run(session, &part, &instances)
        }
        Request::Measure { entities, rev } => {
            crate::measure::run(session, &entities, rev.as_deref())
        }
        Request::Clearance {
            part,
            instances,
            below,
        } => crate::clearance::run(session, &part, &instances, below.as_deref()),
        Request::SheetGet { part, solid, rev } => {
            crate::sheet_metal::report::get(session, &part, solid.as_deref(), rev.as_deref())
        }
        Request::FeatureFacesGet { face, rev } => {
            crate::tweak::feature_faces_get(session, &face, rev.as_deref())
        }
        Request::BlendsGet {
            part,
            solid,
            tolerance,
            below,
            rev,
        } => crate::blends::get(
            session,
            &part,
            solid.as_deref(),
            tolerance.as_deref(),
            below.as_deref(),
            rev.as_deref(),
        ),
        Request::FlatGet {
            part,
            solid,
            rev,
            fixed,
            bends,
        } => crate::sheet_metal::flat::get(
            session,
            &part,
            solid.as_deref(),
            rev.as_deref(),
            &crate::sheet_metal::flat::Options { fixed, bends },
        ),
        Request::Mass { of, tolerance, rev } => {
            crate::mass::run(session, &of, tolerance, rev.as_deref())
        }
        Request::DepsAdd {
            url,
            path,
            name,
            expected_revision,
        } => crate::deps::add(
            session,
            &url,
            &path,
            name.as_deref(),
            expected_revision.as_deref(),
        ),
        Request::DepsFetch { names } => crate::deps::fetch(session, &names),
        Request::DepsList {} => crate::deps::list(session),
        Request::DepsOutdated {} => crate::deps::outdated(session),
        Request::DepsUses { rev } => crate::deps::uses(session, rev.as_deref()),
        Request::DepsRemove {
            name,
            expected_revision,
        } => crate::deps::remove(session, &name, expected_revision.as_deref()),
        Request::DepsUpdate {
            names,
            part,
            to,
            offline,
            dry_run,
            expected_revision,
        } => crate::deps::update(
            session,
            crate::deps::Update {
                names: &names,
                part: part.as_deref(),
                to: to.as_deref(),
                offline,
                dry_run,
            },
            expected_revision.as_deref(),
        ),
        Request::DepsParts { name, part } => crate::deps::parts(session, &name, part.as_deref()),
        Request::InsertUpdate {
            part,
            feature,
            to,
            dry_run,
            expected_revision,
        } => crate::insert::update(
            session,
            crate::insert::Update {
                part: &part,
                feature: &feature,
                to: to.as_deref(),
                dry_run,
            },
            expected_revision.as_deref(),
        ),
        Request::MaterialsList {} => crate::materials::list(session),
        Request::EquationsList { part, all, rev } => crate::equations::list(
            session,
            &crate::equations::Ask { part, all },
            rev.as_deref(),
        ),
        Request::MaterialsSet {
            name,
            density,
            colour,
            default,
            expected_revision,
        } => crate::materials::set(
            session,
            &name,
            &density,
            colour.as_deref(),
            default,
            expected_revision.as_deref(),
        ),
        Request::MaterialsDelete {
            name,
            expected_revision,
        } => crate::materials::delete(session, &name, expected_revision.as_deref()),
        Request::AppearanceGet {
            part,
            participant,
            rev,
        } => view::appearance_get(session, live, part, participant, rev),
        Request::AppearanceSet {
            entities,
            colour,
            material,
            opacity,
            unset,
            file_styles,
            expected_revision,
        } => view::appearance_set(
            session,
            entities,
            colour,
            material,
            opacity,
            unset,
            file_styles,
            expected_revision,
        ),
        Request::AppearanceClear {
            entities,
            expected_revision,
        } => crate::appearance::clear(session, &entities, expected_revision.as_deref()),
        Request::AssemblySolve {
            part,
            moves,
            component,
            rev,
            mates,
        } => assembly::assembly_solve(session, part, moves, component, rev, mates),
        Request::FeatureShow { part, id, rev } => feature::feature_show(session, part, id, rev),
        Request::MergeStatus {} => crate::validate::status(session),
        Request::MergePreview { rev } => crate::validate::preview(session, &rev),
        Request::WorktreeList {} => crate::worktree::list(session, live.as_deref()),
        Request::BranchList {} => crate::branch::list(session),
        Request::BranchSwitch {
            branch,
            expected_revision,
        } => crate::branch::switch(session, live, me, &branch, expected_revision.as_deref()),
        Request::MergeResolve {
            resolutions,
            expected_revisions,
        } => crate::validate::resolve(session, &resolutions, &expected_revisions),
        Request::DaemonStatus {} | Request::DaemonShutdown {} => Err(Error::new(
            Code::Usage,
            "only a running daemon answers daemon requests",
        )),
        // A daemon answers this itself, without the lock; in process
        // nothing else is running.
        Request::RequestCancel { token, .. } => Ok(json!({"cancelled": false, "token": token})),
        Request::Import {
            path,
            part,
            into,
            product,
            dry_run,
            expected_revision,
            update,
            solid,
            flatten,
        } => crate::import::import(
            session,
            crate::import::Ask {
                path,
                part,
                into,
                product,
                dry_run,
                expected: expected_revision,
                update,
                solid,
                flatten,
            },
        ),
        Request::ImportReport { file, detail } => {
            crate::import::transfer_report(session, &file, detail)
        }
        Request::ImportConvert {
            part,
            feature,
            reference,
            expected_revision,
        } => crate::import::convert::convert(
            session,
            crate::import::convert::Ask {
                part,
                feature,
                reference,
                expected: expected_revision,
            },
        ),
        // A rebuild's report is written as text: `answer_live` handles it.
        Request::Rebuild { .. } => Err(Error::new(Code::Internal, "rebuild answers as text")),
        Request::Inspect {
            part,
            feature,
            entity,
            rev,
        } => view::inspect_request(session, part, feature, entity, rev),
        Request::CheckFormat {} => check_format(session),
        Request::Check {
            part,
            against,
            results,
            interference,
        } => crate::check::run(
            session,
            part.as_deref(),
            against.as_deref(),
            results,
            interference,
        ),
        Request::Diff {
            from,
            to,
            part,
            model,
            positions,
            all,
        } => git::diff_request(session, from, to, part, model, positions, all),
        Request::SessionHello {
            kind,
            name,
            participant,
        } => session::session_hello(live, kind, name, participant),
        // With no daemon, nobody is attached.
        Request::ParticipantList {} => {
            Ok(live.map_or_else(|| json!({"participants": []}), |l| l.list()))
        }
        // A daemon's connection answers this itself: it needs the socket.
        Request::Subscribe { .. } => session::subscribe(live),
        Request::SelectionGet { participant } => view::selection_get(live, participant),
        Request::SelectionSet { entities, mode } => {
            view::selection_set(session, live, me, entities, mode)
        }
        Request::CameraGet { participant } => view::camera_get(live, participant),
        Request::CameraSet {
            camera,
            view,
            fit,
            normal_to,
            part,
        } => view::camera_set(session, live, me, camera, view, fit, normal_to, part),
        Request::ViewList { part } => crate::views::list(session, part.as_deref()),
        Request::ViewSave {
            part,
            view,
            camera,
            participant,
            expected_revision,
        } => view::view_save(
            session,
            live,
            me,
            part,
            view,
            camera,
            participant,
            expected_revision,
        ),
        Request::ViewDelete {
            part,
            view,
            expected_revision,
        } => crate::views::delete(session, &part, &view, expected_revision.as_deref()),
        Request::ViewRename {
            part,
            from,
            to,
            expected_revision,
        } => crate::views::rename(session, &part, &from, &to, expected_revision.as_deref()),
        Request::CompareGet { participant } => git::compare_get(session, live, participant),
        Request::CompareSet {
            rev,
            off,
            participant,
        } => git::compare_set(session, live, me, rev, off, participant),
        Request::ViewFocus {
            part,
            project,
            participant,
        } => view::view_focus(session, live, me, part, project, participant),
        Request::VisibilitySet {
            hide,
            show,
            isolate,
            all,
            participant,
        } => view::visibility_set(session, live, me, hide, show, isolate, all, participant),
        Request::DrawingList { part } => crate::drawing::request::list(session, part.as_deref()),
        Request::DrawingCreate {
            part,
            drawing,
            name,
            template,
            size,
            views,
            expected_revision,
        } => crate::drawing::request::create(
            session,
            &part,
            &drawing,
            &crate::drawing::request::Create {
                name,
                template,
                size,
                views,
            },
            expected_revision.as_deref(),
        ),
        Request::DrawingEdit {
            part,
            drawing,
            ops,
            expected_revision,
        } => crate::drawing::request::edit(
            session,
            &part,
            &drawing,
            &ops,
            expected_revision.as_deref(),
        ),
        Request::DrawingDelete {
            part,
            drawing,
            expected_revision,
        } => {
            crate::drawing::request::delete(session, &part, &drawing, expected_revision.as_deref())
        }
        Request::DrawingRename {
            part,
            drawing,
            to,
            expected_revision,
        } => crate::drawing::request::rename(
            session,
            &part,
            &drawing,
            &to,
            expected_revision.as_deref(),
        ),
        Request::DrawingLayout {
            part,
            drawing,
            dry_run,
            expected_revision,
        } => crate::drawing::request::layout(
            session,
            &part,
            &drawing,
            dry_run,
            expected_revision.as_deref(),
        ),
        Request::DrawingGet {
            part,
            drawing,
            rev,
            sheet,
            items,
        } => session.at(rev.as_deref()).and_then(|(commit, s)| {
            noted(
                crate::drawing::request::get(s, &part, &drawing, sheet.as_deref(), items),
                commit,
            )
        }),
        Request::DrawingDescribe {
            part,
            drawing,
            rev,
            sheet,
        } => session.at(rev.as_deref()).and_then(|(commit, s)| {
            noted(
                crate::drawing::request::describe(s, &part, &drawing, sheet.as_deref()),
                commit,
            )
        }),
        Request::DrawingLint {
            part,
            drawing,
            rev,
            sheet,
            explain,
        } => session.at(rev.as_deref()).and_then(|(commit, s)| {
            noted(
                crate::drawing::request::lint(
                    s,
                    part.as_deref(),
                    drawing.as_deref(),
                    sheet.as_deref(),
                    explain.as_deref(),
                ),
                commit,
            )
        }),
        Request::DrawingLintConfigure {
            weights,
            severity,
            expected_revision,
        } => crate::drawing::request::lint_configure(
            session,
            &weights,
            &severity,
            expected_revision.as_deref(),
        ),
        Request::ExplodeList { part } => crate::explode::list(session, part.as_deref()),
        Request::ExplodeSave {
            part,
            explode,
            steps,
            expected_revision,
        } => crate::explode::save(
            session,
            &part,
            &explode,
            &steps,
            expected_revision.as_deref(),
        ),
        Request::ParticipantMode { mode } => session::participant_mode(live, me, mode),
        Request::AnnotationApply {
            id,
            keep_open,
            expect,
            author,
        } => annotation::annotation_apply(session, live, me, id, keep_open, expect, author),
        Request::AnnotationCreate {
            pins,
            comment,
            camera,
            with_view,
            marks,
            view: named,
            author,
        } => annotation::annotation_create(
            session, live, me, pins, comment, camera, with_view, marks, named, author,
        ),
        Request::AnnotationList {
            status,
            part,
            by,
            open,
        } => crate::annotation::list(session, &status, part.as_deref(), by.as_deref(), open),
        Request::AnnotationShow { id } => crate::annotation::show(session, &id),
        Request::AnnotationReply {
            id,
            text,
            expect,
            author,
        } => annotation::annotation_reply(session, live, me, id, text, expect, author),
        Request::AnnotationClaim { id, expect, author } => {
            annotation::annotation_claim(session, live, me, id, expect, author)
        }
        Request::AnnotationResolve {
            id,
            note,
            expect,
            author,
        } => annotation::annotation_resolve(session, live, me, id, note, expect, author),
        Request::AnnotationReject {
            id,
            reason,
            expect,
            author,
        } => annotation::annotation_reject(session, live, me, id, reason, expect, author),
        Request::AnnotationReopen { id, expect, .. } => {
            crate::annotation::reopen(session, &id, expect.as_deref())
        }
        Request::AnnotationEdit {
            id,
            comment,
            pins,
            marks,
            no_marks,
            expect,
            author,
        } => annotation::annotation_edit(
            session, live, me, id, comment, pins, marks, no_marks, expect, author,
        ),
        Request::AnnotationDelete { id, expect, author } => {
            annotation::annotation_delete(session, live, me, id, expect, author)
        }
        Request::AnnotationPropose {
            request,
            comment,
            pins,
            rough,
            author,
        } => {
            annotation::annotation_propose(session, live, me, request, comment, pins, rough, author)
        }
        Request::ExplodeDelete {
            part,
            explode,
            expected_revision,
        } => assembly::explode_delete(session, live, part, explode, expected_revision),
        Request::ExplodeShow {
            part,
            explode,
            fraction,
            off,
            participant,
        } => assembly::explode_show(session, live, me, part, explode, fraction, off, participant),
        // Where `section.set` would cut: `participant`, else the one
        // human, else the caller.
        Request::SectionGet { participant } => view::section_get(live, me, participant),
        Request::SectionSet {
            planes,
            add,
            off,
            participant,
        } => view::section_set(session, live, me, planes, add, off, participant),
        Request::HighlightSet {
            entities,
            style,
            label,
        } => view::highlight_set(session, live, me, entities, style, label),
        Request::HighlightClear { style } => view::highlight_clear(live, me, style),
        Request::ActivityLog { limit, before } => session::activity_log(live, limit, before),
        Request::Undo { entry } => session::undo(session, live, me, entry),
        Request::Redo { entry } => session::redo(session, live, me, entry),
        Request::MeshGet {
            part,
            feature,
            features,
            quality,
            transfer: _,
            rev,
            flat: Some(solid),
        } => view::mesh_get_flat(session, part, feature, features, quality, rev, solid),
        Request::MeshGet {
            part,
            feature,
            features,
            quality,
            transfer: _,
            rev,
            flat: None,
        } => view::mesh_get_request(session, part, feature, features, quality, rev),
        // A render's answer is bytes: `answer_live` handles it.
        Request::Render { .. } => Err(Error::new(Code::Internal, "render answers with bytes")),
        Request::Export { .. } => Err(Error::new(Code::Internal, "export answers with bytes")),
        Request::ReleasePackage { .. } => Err(Error::new(
            Code::Internal,
            "release.package answers with bytes",
        )),
        Request::DrawingRender { .. } | Request::DrawingExport { .. } => Err(Error::new(
            Code::Internal,
            "drawings render and export with bytes",
        )),
        // The session is current: doctor reuses its rebuilds (#125).
        Request::Doctor {} => session::doctor(session),
        Request::DoctorRepair { repairs, params } => {
            session::doctor_repair(session, repairs, params)
        }
        Request::HistoryLog { part, limit } => session::history_log(session, part, limit),
        Request::CacheStats {} => Ok(cache_stats(session.disk().map(|d| d.as_ref()), None)),
        Request::CacheClear {} => cache::cache_clear(session),
        Request::CacheLogin { token } => cache::cache_login_request(session, token),
        Request::CacheLogout {} => cache::cache_logout(session),
        Request::CacheWarm { part } => cache_warm(session, part.as_deref()),
        Request::ConfigGet { key, scope } => session::config_get(session, key, scope),
        Request::ConfigSet {
            key,
            value,
            scope,
            expected_revision,
        } => session::config_set(session, live, key, value, scope, expected_revision),
        Request::AppOpen { participant } => session::app_open(live, participant),
        Request::ActivitySet { status, entities } => {
            session::activity_set(session, live, me, status, entities)
        }
    }
}
