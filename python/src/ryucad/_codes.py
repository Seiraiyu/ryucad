# Generated from ryu_doc::error::Code by crates/ryucad/tests/python.rs:
# never edit it by hand. After adding a code, run
#   UPDATE_PYTHON=1 cargo test -p ryucad --test python
"""Every stable error code ryucad's products answer, and each one's exit
class: 1 the agent can fix it, 2 usage, 3 not a usable project, 4 not the
agent's fault."""

from enum import Enum


class Code(str, Enum):
    """A stable error code: never renamed or reused."""

    INVALID_JSON = "invalid_json"
    INVALID_DOCUMENT = "invalid_document"
    INVALID_QUANTITY = "invalid_quantity"
    INVALID_ID = "invalid_id"
    INVALID_PLACEMENT = "invalid_placement"
    UNKNOWN_FEATURE_TYPE = "unknown_feature_type"
    INVALID_PARAM = "invalid_param"
    DUPLICATE_ID = "duplicate_id"
    NOT_FOUND = "not_found"
    CONFLICT = "conflict"
    REBUILD_FAILED = "rebuild_failed"
    PROJECT_EXISTS = "project_exists"
    PROJECT_NOT_FOUND = "project_not_found"
    FORMAT_TOO_NEW = "format_too_new"
    USAGE = "usage"
    IO = "io"
    DAEMON = "daemon"
    INTERNAL = "internal"
    NO_LIVE_SESSION = "no_live_session"
    AMBIGUOUS_PARTICIPANT = "ambiguous_participant"
    NOTHING_TO_UNDO = "nothing_to_undo"
    UNDO_CONFLICT = "undo_conflict"
    UNKNOWN_SETTING = "unknown_setting"
    INVALID_SETTING = "invalid_setting"
    NOT_A_GIT_REPO = "not_a_git_repo"
    MESH_FAILED = "mesh_failed"
    RENDER_TOO_LARGE = "render_too_large"
    PROJECT_DAMAGED = "project_damaged"
    REPAIR_CONFLICT = "repair_conflict"
    APP_NOT_INSTALLED = "app_not_installed"
    UNKNOWN_PARAMETER = "unknown_parameter"
    UNIT_MISMATCH = "unit_mismatch"
    PARAMETER_CYCLE = "parameter_cycle"
    HAS_DEPENDENTS = "has_dependents"
    SKETCH_UNSOLVED = "sketch_unsolved"
    SKETCH_OVERCONSTRAINED = "sketch_overconstrained"
    PROFILE_INVALID = "profile_invalid"
    INVALID_EXTENT = "invalid_extent"
    REFERENCE_FORWARD = "reference_forward"
    REFERENCE_WRONG_KIND = "reference_wrong_kind"
    DEPENDENCY_FAILED = "dependency_failed"
    REFERENCE_UNRESOLVED = "reference_unresolved"
    REFERENCE_SPLIT = "reference_split"
    REFERENCE_FOLLOWED = "reference_followed"
    PROJECTION_UNSUPPORTED = "projection_unsupported"
    REV_NOT_FOUND = "rev_not_found"
    CHECK_FAILED = "check_failed"
    MERGE_CONFLICT = "merge_conflict"
    NOT_MERGING = "not_merging"
    MERGE_UNSUPPORTED = "merge_unsupported"
    CONFLICT_NOT_FOUND = "conflict_not_found"
    PROPERTY_UNKNOWN = "property_unknown"
    PROPERTY_INVALID = "property_invalid"
    PART_NUMBER_INVALID = "part_number_invalid"
    PART_NUMBER_DUPLICATE = "part_number_duplicate"
    PART_ID_RETIRED = "part_id_retired"
    REVISION_SCHEME_LOCKED = "revision_scheme_locked"
    OPERATION_NO_EFFECT = "operation_no_effect"
    BODY_EMPTIED = "body_emptied"
    BOOLEAN_FAILED = "boolean_failed"
    BODY_CONSUMED = "body_consumed"
    RELEASE_NOT_READY = "release_not_ready"
    NOTHING_TO_RELEASE = "nothing_to_release"
    RELEASE_PENDING = "release_pending"
    RELEASE_FOLDER_UNSAFE = "release_folder_unsafe"
    RELEASE_TAG_CLASH = "release_tag_clash"
    REVIEW_UNAUTHENTICATED = "review_unauthenticated"
    REVIEW_NO_REMOTE = "review_no_remote"
    REVIEW_UNREACHABLE = "review_unreachable"
    REVIEW_NOT_INSTALLED = "review_not_installed"
    REVIEW_NOT_FOUND = "review_not_found"
    REVIEW_FORBIDDEN = "review_forbidden"
    REVIEW_PENDING = "review_pending"
    REVIEW_HEAD_MOVED = "review_head_moved"
    REVIEW_BASE_MOVED = "review_base_moved"
    REVIEW_FORK_NOT_INSTALLED = "review_fork_not_installed"
    REVIEW_LIMIT_EXCEEDED = "review_limit_exceeded"
    REVIEW_REFERENCE_INVALID = "review_reference_invalid"
    REVIEW_FAILED = "review_failed"
    TARGETS_MIXED = "targets_mixed"
    EDGE_LISTED_TWICE = "edge_listed_twice"
    BLEND_FAILED = "blend_failed"
    WRONG_PART_KIND = "wrong_part_kind"
    ASSEMBLY_CYCLE = "assembly_cycle"
    LABEL_NOT_FOUND = "label_not_found"
    LABEL_NOT_OFFICIAL = "label_not_official"
    INSTANCE_NOT_RELEASED = "instance_not_released"
    INSERT_CYCLE = "insert_cycle"
    INSERT_SOURCE_EMPTY = "insert_source_empty"
    INSERT_SOURCE_FAILED = "insert_source_failed"
    INSERT_OBSOLETE = "insert_obsolete"
    INSERT_BEHIND = "insert_behind"
    INSERT_NOT_RELEASED = "insert_not_released"
    BODY_DELETED = "body_deleted"
    INSTANCE_FIXED = "instance_fixed"
    CONNECTOR_UNSUPPORTED = "connector_unsupported"
    ASSEMBLY_UNSOLVED = "assembly_unsolved"
    ASSEMBLY_OVERCONSTRAINED = "assembly_overconstrained"
    MATE_OUT_OF_LIMITS = "mate_out_of_limits"
    IMPORT_FILE_MISSING = "import_file_missing"
    IMPORT_FILE_CHANGED = "import_file_changed"
    STEP_UNREADABLE = "step_unreadable"
    IMPORT_PRODUCT_MISSING = "import_product_missing"
    IMPORT_BODY_MISSING = "import_body_missing"
    IMPORT_EXTERNAL_MISSING = "import_external_missing"
    IMPORT_EXTERNAL_AMBIGUOUS = "import_external_ambiguous"
    IMPORT_FAILED = "import_failed"
    EXPORT_FAILED = "export_failed"
    EXPORT_UNREPRESENTABLE = "export_unrepresentable"
    REVIEW_NOT_IN_BETA = "review_not_in_beta"
    REVIEW_INVALID_REQUEST = "review_invalid_request"
    REVIEW_MERGE_UNAVAILABLE = "review_merge_unavailable"
    REVIEW_GITHUB_UNREACHABLE = "review_github_unreachable"
    BOM_ITEM_MISSING = "bom_item_missing"
    BOM_ITEM_DUPLICATE = "bom_item_duplicate"
    BOM_ITEM_UNUSED = "bom_item_unused"
    MEASURE_FAILED = "measure_failed"
    VIEW_NOT_FOUND = "view_not_found"
    APPEARANCE_UNRESOLVED = "appearance_unresolved"
    EXPLODE_NOT_FOUND = "explode_not_found"
    EXPLODE_UNRESOLVED = "explode_unresolved"
    MOTION_NOT_FOUND = "motion_not_found"
    MOTION_UNRESOLVED = "motion_unresolved"
    MOTION_PAST_LIMIT = "motion_past_limit"
    MOTION_BLOCKED = "motion_blocked"
    MOTION_INTERFERES = "motion_interferes"
    LIBRARY_MISSING = "library_missing"
    LIBRARY_NESTED = "library_nested"
    LIBRARY_UNIT_INVALID = "library_unit_invalid"
    LIBRARY_PIN_INVALID = "library_pin_invalid"
    LIBRARY_GRAPHIC_INVALID = "library_graphic_invalid"
    LIBRARY_OUT_OF_RANGE = "library_out_of_range"
    LIBRARY_PAD_INVALID = "library_pad_invalid"
    LIBRARY_LAYER_UNKNOWN = "library_layer_unknown"
    LIBRARY_COURTYARD_MISSING = "library_courtyard_missing"
    MODEL_FILE_MISSING = "model_file_missing"
    MODEL_FILE_CHANGED = "model_file_changed"
    MODEL_UNREADABLE = "model_unreadable"
    PIN_NOT_FOUND = "pin_not_found"
    PIN_WITHOUT_PAD = "pin_without_pad"
    PAD_WITHOUT_PIN = "pad_without_pin"
    PAD_MAPPED_TWICE = "pad_mapped_twice"
    BLOCK_CYCLE = "block_cycle"
    PORT_NOT_FOUND = "port_not_found"
    NET_NOT_FOUND = "net_not_found"
    NET_NAME_DUPLICATE = "net_name_duplicate"
    NET_HAS_PINS = "net_has_pins"
    PIN_CONFLICT = "pin_conflict"
    POWER_UNDRIVEN = "power_undriven"
    PIN_UNCONNECTED = "pin_unconnected"
    NC_CONNECTED = "nc_connected"
    NET_SINGLE_PIN = "net_single_pin"
    INPUT_UNDRIVEN = "input_undriven"
    REFDES_DUPLICATE = "refdes_duplicate"
    REFDES_MISSING = "refdes_missing"
    FOOTPRINT_MISSING = "footprint_missing"
    PAIR_UNMATCHED = "pair_unmatched"
    INTENT_CONFLICT = "intent_conflict"
    LIBRARY_FAULT = "library_fault"
    WAIVER_UNUSED = "waiver_unused"
    KICAD_UNREADABLE = "kicad_unreadable"
    KICAD_LEGACY_UNSUPPORTED = "kicad_legacy_unsupported"
    IMPORT_IGNORED = "import_ignored"
    IMPORT_ROUNDED = "import_rounded"
    IMPORT_POWER_SKIPPED = "import_power_skipped"
    IMPORT_DEMORGAN_DROPPED = "import_demorgan_dropped"
    IMPORT_LAYER_UNMAPPED = "import_layer_unmapped"
    IMPORT_MODEL_SKIPPED = "import_model_skipped"
    IMPORT_UNRESOLVED = "import_unresolved"
    IMPORT_VERSION_NEWER = "import_version_newer"
    WATCH_UNAVAILABLE = "watch_unavailable"
    MEMORY_BUDGET_TOO_SMALL = "memory_budget_too_small"
    EXPORT_MESH_FAULTS = "export_mesh_faults"
    EXPORT_TOO_LARGE = "export_too_large"
    NOT_A_BOARD = "not_a_board"
    ODB_UNREADABLE = "odb_unreadable"
    ODB_VERSION_UNSUPPORTED = "odb_version_unsupported"
    ODB_STEP_AMBIGUOUS = "odb_step_ambiguous"
    ODB_STEP_NOT_FOUND = "odb_step_not_found"
    ARCHIVE_UNSAFE = "archive_unsafe"
    BOARD_INPUT_MISSING = "board_input_missing"
    BOARD_INPUT_CHANGED = "board_input_changed"
    ODB_LAYER_UNREADABLE = "odb_layer_unreadable"
    ODB_SYMBOL_UNKNOWN = "odb_symbol_unknown"
    ODB_SYMBOL_CYCLE = "odb_symbol_cycle"
    ODB_COMPONENT_INVALID = "odb_component_invalid"
    ODB_EDA_INVALID = "odb_eda_invalid"
    ODB_FILE_INVALID = "odb_file_invalid"
    ODB_IGNORED = "odb_ignored"
    ODB_ROUNDED = "odb_rounded"
    ODB_ARC_INCONSISTENT = "odb_arc_inconsistent"
    ODB_FONT_MISSING_CHAR = "odb_font_missing_char"
    ODB_TEXT_DYNAMIC = "odb_text_dynamic"
    ODB_BARCODE_SKIPPED = "odb_barcode_skipped"
    ODB_INFO_MISSING = "odb_info_missing"
    NET_OPEN = "net_open"
    NET_SHORT = "net_short"
    NETLIST_POINT_OFF_COPPER = "netlist_point_off_copper"
    NET_CLAIM_MISMATCH = "net_claim_mismatch"
    NETLIST_FROM_PINS = "netlist_from_pins"
    NETLIST_MISSING = "netlist_missing"
    COPPER_INCOMPLETE = "copper_incomplete"
    COPPER_CONTACT_UNCERTAIN = "copper_contact_uncertain"
    CACHE_REMOTE_UNREACHABLE = "cache_remote_unreachable"
    CACHE_REMOTE_REFUSED = "cache_remote_refused"
    CACHE_ENTRY_BAD = "cache_entry_bad"
    WORKTREE_NOT_FOUND = "worktree_not_found"
    WORKTREE_DIRTY = "worktree_dirty"
    CHECKOUT_DIRTY = "checkout_dirty"
    BRANCH_IN_WORKTREE = "branch_in_worktree"
    MERGE_IN_PROGRESS = "merge_in_progress"
    GIT_FAILED = "git_failed"
    DEPENDENCY_INVALID = "dependency_invalid"
    DEPENDENCY_MISSING = "dependency_missing"
    DEPENDENCY_NOT_FETCHED = "dependency_not_fetched"
    DEPENDENCY_FETCH_FAILED = "dependency_fetch_failed"
    DEPENDENCY_PROJECT_MISMATCH = "dependency_project_mismatch"
    DEPENDENCY_REWRITTEN = "dependency_rewritten"
    DEPENDENCY_UNLOCKED = "dependency_unlocked"
    INSTANCE_FOREIGN_WORKING = "instance_foreign_working"
    INSTANCE_OUTDATED = "instance_outdated"
    LOCK_INVALID = "lock_invalid"
    LOCK_UNUSED = "lock_unused"
    PROJECT_ID_DUPLICATE = "project_id_duplicate"
    LAYOUT_STALE = "layout_stale"
    LAYOUT_INVALID = "layout_invalid"
    LAYOUT_RULE = "layout_rule"
    SHEET_FULL = "sheet_full"
    INSTRUCTION_CONFLICT = "instruction_conflict"
    INSTRUCTION_UNRESOLVED = "instruction_unresolved"
    REFDES_FORMAT_INVALID = "refdes_format_invalid"
    SCH_CROSSINGS = "sch_crossings"
    SCH_BENDS = "sch_bends"
    SCH_LONG_WIRE = "sch_long_wire"
    SCH_LABEL_WHERE_WIRE = "sch_label_where_wire"
    SCH_WIRE_WHERE_LABEL = "sch_wire_where_label"
    SCH_DENSITY = "sch_density"
    SCH_FLOW_REVERSED = "sch_flow_reversed"
    SCH_PIN_OFF_GRID = "sch_pin_off_grid"
    SCH_IDIOM_MISSED = "sch_idiom_missed"
    PROJECT_ID_MISSING = "project_id_missing"
    CANCELLED = "cancelled"
    APPROVAL_RULE_INVALID = "approval_rule_invalid"
    PARAMETER_SHADOWS = "parameter_shadows"
    PARAMETER_UNUSED = "parameter_unused"
    DATUM_DEGENERATE = "datum_degenerate"
    DATUM_NO_INTERSECTION = "datum_no_intersection"
    DATUM_AMBIGUOUS = "datum_ambiguous"
    IMPORT_BUDGET_EXCEEDED = "import_budget_exceeded"
    PATH_DISCONNECTED = "path_disconnected"
    PATH_NOT_TANGENT = "path_not_tangent"
    PATH_SEGMENT_TWICE = "path_segment_twice"
    PATH_BRANCHES = "path_branches"
    PATH_INVALID = "path_invalid"
    HELIX_INVALID = "helix_invalid"
    LAYOUT_CONFLICT = "layout_conflict"
    SCH_POWER_SIDEWAYS = "sch_power_sideways"
    SCH_GROUND_SIDEWAYS = "sch_ground_sideways"
    SCH_LABEL_VERTICAL = "sch_label_vertical"
    SCH_JOG = "sch_jog"
    SCH_BYPASS_APART = "sch_bypass_apart"
    SCH_PIN_NAME_OUTSIDE = "sch_pin_name_outside"
    SCH_PORT_BENT = "sch_port_bent"
    SCH_FLOATING = "sch_floating"
    SCH_RESISTOR_STYLE = "sch_resistor_style"
    SCH_OUT_OF_LINE = "sch_out_of_line"
    SCH_BANK_NOT_IN_ROW = "sch_bank_not_in_row"
    SCH_TOPOLOGY = "sch_topology"
    SCH_PART_FLIPPED = "sch_part_flipped"
    SCH_WIRE_WRAPS = "sch_wire_wraps"
    SWEEP_FAILED = "sweep_failed"
    LOFT_FAILED = "loft_failed"
    SHELL_FAILED = "shell_failed"
    OFFSET_FAILED = "offset_failed"
    MIRROR_FAILED = "mirror_failed"
    TWEAK_FAILED = "tweak_failed"
    EDIT_BUDGET_EXCEEDED = "edit_budget_exceeded"
    REGION_UNRESOLVED = "region_unresolved"
    SHEET_RESIZED = "sheet_resized"
    SCH_PAPER_FIT = "sch_paper_fit"
    SCH_FUNCTION_SPLIT = "sch_function_split"
    SCH_REGION_UNTITLED = "sch_region_untitled"
    WRONG_BODY_KIND = "wrong_body_kind"
    PARAMETER_INVALID = "parameter_invalid"
    PART_UNIDENTIFIED = "part_unidentified"
    PART_UNSOURCED = "part_unsourced"
    PARTS_SOURCE_UNREACHABLE = "parts_source_unreachable"
    PARTS_SOURCE_REFUSED = "parts_source_refused"
    PARTS_NOT_FOUND = "parts_not_found"
    PARTS_CACHE_STALE = "parts_cache_stale"
    PARTS_MISMATCH = "parts_mismatch"
    PART_OUT_OF_STOCK = "part_out_of_stock"
    PART_EXTENDED = "part_extended"
    PART_NO_DATA = "part_no_data"
    PARTS_RATE_LIMITED = "parts_rate_limited"
    PART_LIFECYCLE = "part_lifecycle"
    SHEET_METAL_FAILED = "sheet_metal_failed"
    FILL_FAILED = "fill_failed"
    EXTRACT_FAILED = "extract_failed"
    SEW_FAILED = "sew_failed"
    THICKEN_FAILED = "thicken_failed"
    ANNOTATION_NOT_FOUND = "annotation_not_found"
    ANNOTATION_STALE = "annotation_stale"
    ANNOTATION_CLAIMED = "annotation_claimed"
    PARTICIPANT_MODE = "participant_mode"
    REFERENCE_BODY = "reference_body"
    REFERENCE_NOT_SOLID = "reference_not_solid"
    REFERENCE_FACES_UNMESHED = "reference_faces_unmeshed"
    REFERENCE_FACES_DROPPED = "reference_faces_dropped"
    REFERENCE_FACES_SPLIT = "reference_faces_split"
    REFERENCE_UNORIENTED = "reference_unoriented"
    EXPORT_REFERENCE_MESH_ONLY = "export_reference_mesh_only"
    DRAFT_ANGLE_VARIES = "draft_angle_varies"
    THREAD_SIZE_MISMATCH = "thread_size_mismatch"
    HOLE_PLACE_OFF_FACE = "hole_place_off_face"
    HOLE_PLACE_AMBIGUOUS = "hole_place_ambiguous"
    EXPORT_SHEET_BREP_SKIPPED = "export_sheet_brep_skipped"
    DRAWING_NOT_FOUND = "drawing_not_found"
    DRAWING_VIEW_FAILED = "drawing_view_failed"
    DRAWING_VIEW_UNCERTAIN = "drawing_view_uncertain"
    DRAWING_REFERENCE_UNRESOLVED = "drawing_reference_unresolved"
    DRAWING_REFERENCE_NOT_SHOWN = "drawing_reference_not_shown"
    TEMPLATE_NOT_FOUND = "template_not_found"
    PACKAGES_FORMAT_UNKNOWN = "packages_format_unknown"
    PACKAGE_FAILED = "package_failed"
    PACKAGE_INVALID = "package_invalid"
    RELEASE_OBSOLETE = "release_obsolete"
    SCH_OFF_SHEET_LOCAL = "sch_off_sheet_local"
    SCH_BUS_UNBUNDLED = "sch_bus_unbundled"
    STANDARDS_INVALID = "standards_invalid"
    SYM_PIN_OFF_GRID = "sym_pin_off_grid"
    SYM_PIN_SIDE = "sym_pin_side"
    SYM_PIN_GROUP = "sym_pin_group"
    SYM_PIN_LENGTH = "sym_pin_length"
    SYM_SHAPE = "sym_shape"
    SYM_BODY_MARGIN = "sym_body_margin"
    SYM_UNITS = "sym_units"
    STYLE_INVALID = "style_invalid"
    STYLE_LOCKED = "style_locked"
    SCH_CONTRAST = "sch_contrast"
    SCH_OVERRIDE_UNUSED = "sch_override_unused"
    ANNOTATE_CONFLICT = "annotate_conflict"
    GITATTRIBUTES_MISSING = "gitattributes_missing"
    GIT_DRIVERS_MISSING = "git_drivers_missing"
    NON_CANONICAL = "non_canonical"
    UNKNOWN_FILE = "unknown_file"
    MODEL_ORPHANED = "model_orphaned"
    UNMERGED_PATH = "unmerged_path"
    GIT_HOOK_MISSING = "git_hook_missing"
    GIT_HOOK_FOREIGN = "git_hook_foreign"
    LAYOUT_PENDING = "layout_pending"
    SHEET_NOT_FOUND = "sheet_not_found"
    REF_UNRESOLVED = "ref_unresolved"
    REFDES_CLASH = "refdes_clash"
    NAME_CLASH = "name_clash"
    MERGE_FAULTS = "merge_faults"
    COMPONENT_NOT_RELEASED = "component_not_released"
    COMPONENT_OBSOLETE = "component_obsolete"
    COMPONENT_OUTDATED = "component_outdated"
    LIBRARY_FOREIGN_WORKING = "library_foreign_working"
    BOM_UNIDENTIFIED = "bom_unidentified"
    BOARD_NOT_FOUND = "board_not_found"
    BOARD_EXISTS = "board_exists"
    BOARD_DESIGN_NOT_FOUND = "board_design_not_found"
    BOARD_OUTLINE_INVALID = "board_outline_invalid"
    BOARD_OUT_OF_RANGE = "board_out_of_range"
    BOARD_REF_AMBIGUOUS = "board_ref_ambiguous"
    BOARD_FOOTPRINT_UNSUPPORTED = "board_footprint_unsupported"
    BOARD_TEXT_MISSING_GLYPH = "board_text_missing_glyph"
    BOARD_NET_NOT_FOUND = "board_net_not_found"
    BOARD_UNPLACED = "board_unplaced"
    BOARD_PLACEMENT_ORPHAN = "board_placement_orphan"
    DRC_CLEARANCE = "drc_clearance"
    DRC_MASK_CLEARANCE = "drc_mask_clearance"
    DRC_TRACK_WIDTH = "drc_track_width"
    DRC_DRILL = "drc_drill"
    DRC_ANNULAR_RING = "drc_annular_ring"
    DRC_HOLE_SPACING = "drc_hole_spacing"
    DRC_EDGE_CLEARANCE = "drc_edge_clearance"
    DRC_COURTYARD_OVERLAP = "drc_courtyard_overlap"
    DRC_COURTYARD_OPEN = "drc_courtyard_open"
    SILK_REF_OVERLAP = "silk_ref_overlap"
    BOARD_STACKUP_INVALID = "board_stackup_invalid"
    BOARD_LAYER_NOT_FOUND = "board_layer_not_found"
    DRC_VIA_SPAN = "drc_via_span"
    DRC_HOLE_CLEARANCE = "drc_hole_clearance"
    BOARD_CLASS_NOT_FOUND = "board_class_not_found"
    BOARD_PRESET_NOT_FOUND = "board_preset_not_found"
    RULE_BELOW_PRESET = "rule_below_preset"
    WAIVER_EXCEEDED = "waiver_exceeded"
    FINDING_NOT_FOUND = "finding_not_found"
    FINDING_NOT_WAIVABLE = "finding_not_waivable"
    PRESET_IN_USE = "preset_in_use"
    PLACEMENT_CONSTRAINT_UNMET = "placement_constraint_unmet"
    PLACEMENT_NO_ROOM = "placement_no_room"
    DRC_KEEPOUT = "drc_keepout"
    BOARD_CONTOUR_INVALID = "board_contour_invalid"
    DRC_FAILED = "drc_failed"
    ITEM_NOT_FOUND = "item_not_found"
    ITEM_CLASS_UNKNOWN = "item_class_unknown"
    PART_NUMBER_UNSCHEMED = "part_number_unschemed"
    PART_NUMBER_DUPLICATE_ELSEWHERE = "part_number_duplicate_elsewhere"
    ITEM_LINK_DUPLICATE = "item_link_duplicate"
    ITEM_LINK_UNRESOLVED = "item_link_unresolved"
    ITEM_NUMBER_MISMATCH = "item_number_mismatch"
    BOM_UNLINKED = "bom_unlinked"
    BOM_LINE_TWICE = "bom_line_twice"
    BOM_CYCLE = "bom_cycle"
    IMPORT_CONFLICT = "import_conflict"
    IMPORT_MISSING = "import_missing"
    IMPORT_MAPPING_INVALID = "import_mapping_invalid"
    DOCUMENT_NOT_FOUND = "document_not_found"
    DOCUMENT_BYTES_MISSING = "document_bytes_missing"
    DOCUMENT_BYTES_CHANGED = "document_bytes_changed"
    DOCUMENT_FILE_UNKNOWN = "document_file_unknown"
    DOCUMENT_LARGE = "document_large"
    DOCUMENT_TOO_LARGE = "document_too_large"
    DOCUMENT_MISSING = "document_missing"
    DOCUMENT_UNRESOLVED = "document_unresolved"
    DOCUMENT_IN_USE = "document_in_use"
    DOCUMENT_RELEASED = "document_released"
    DOCUMENT_UNRELEASED = "document_unreleased"
    ITEM_HISTORY_LABEL_FOREIGN = "item_history_label_foreign"
    PART_NUMBER_TAKEN = "part_number_taken"
    LEDGER_UNAVAILABLE = "ledger_unavailable"


#: Each code's exit class.
EXIT: dict[Code, int] = {
    Code.INVALID_JSON: 1,
    Code.INVALID_DOCUMENT: 1,
    Code.INVALID_QUANTITY: 1,
    Code.INVALID_ID: 1,
    Code.INVALID_PLACEMENT: 1,
    Code.UNKNOWN_FEATURE_TYPE: 1,
    Code.INVALID_PARAM: 1,
    Code.DUPLICATE_ID: 1,
    Code.NOT_FOUND: 1,
    Code.CONFLICT: 1,
    Code.REBUILD_FAILED: 1,
    Code.PROJECT_EXISTS: 1,
    Code.PROJECT_NOT_FOUND: 3,
    Code.FORMAT_TOO_NEW: 3,
    Code.USAGE: 2,
    Code.IO: 4,
    Code.DAEMON: 4,
    Code.INTERNAL: 4,
    Code.NO_LIVE_SESSION: 1,
    Code.AMBIGUOUS_PARTICIPANT: 1,
    Code.NOTHING_TO_UNDO: 1,
    Code.UNDO_CONFLICT: 1,
    Code.UNKNOWN_SETTING: 1,
    Code.INVALID_SETTING: 1,
    Code.NOT_A_GIT_REPO: 1,
    Code.MESH_FAILED: 1,
    Code.RENDER_TOO_LARGE: 1,
    Code.PROJECT_DAMAGED: 3,
    Code.REPAIR_CONFLICT: 1,
    Code.APP_NOT_INSTALLED: 4,
    Code.UNKNOWN_PARAMETER: 1,
    Code.UNIT_MISMATCH: 1,
    Code.PARAMETER_CYCLE: 1,
    Code.HAS_DEPENDENTS: 1,
    Code.SKETCH_UNSOLVED: 1,
    Code.SKETCH_OVERCONSTRAINED: 1,
    Code.PROFILE_INVALID: 1,
    Code.INVALID_EXTENT: 1,
    Code.REFERENCE_FORWARD: 1,
    Code.REFERENCE_WRONG_KIND: 1,
    Code.DEPENDENCY_FAILED: 1,
    Code.REFERENCE_UNRESOLVED: 1,
    Code.REFERENCE_SPLIT: 1,
    Code.REFERENCE_FOLLOWED: 1,
    Code.PROJECTION_UNSUPPORTED: 1,
    Code.REV_NOT_FOUND: 1,
    Code.CHECK_FAILED: 1,
    Code.MERGE_CONFLICT: 1,
    Code.NOT_MERGING: 1,
    Code.MERGE_UNSUPPORTED: 1,
    Code.CONFLICT_NOT_FOUND: 1,
    Code.PROPERTY_UNKNOWN: 1,
    Code.PROPERTY_INVALID: 1,
    Code.PART_NUMBER_INVALID: 1,
    Code.PART_NUMBER_DUPLICATE: 1,
    Code.PART_ID_RETIRED: 1,
    Code.REVISION_SCHEME_LOCKED: 1,
    Code.OPERATION_NO_EFFECT: 1,
    Code.BODY_EMPTIED: 1,
    Code.BOOLEAN_FAILED: 1,
    Code.BODY_CONSUMED: 1,
    Code.RELEASE_NOT_READY: 1,
    Code.NOTHING_TO_RELEASE: 1,
    Code.RELEASE_PENDING: 1,
    Code.RELEASE_FOLDER_UNSAFE: 1,
    Code.RELEASE_TAG_CLASH: 1,
    Code.REVIEW_UNAUTHENTICATED: 1,
    Code.REVIEW_NO_REMOTE: 1,
    Code.REVIEW_UNREACHABLE: 4,
    Code.REVIEW_NOT_INSTALLED: 1,
    Code.REVIEW_NOT_FOUND: 1,
    Code.REVIEW_FORBIDDEN: 1,
    Code.REVIEW_PENDING: 1,
    Code.REVIEW_HEAD_MOVED: 1,
    Code.REVIEW_BASE_MOVED: 1,
    Code.REVIEW_FORK_NOT_INSTALLED: 1,
    Code.REVIEW_LIMIT_EXCEEDED: 1,
    Code.REVIEW_REFERENCE_INVALID: 1,
    Code.REVIEW_FAILED: 4,
    Code.TARGETS_MIXED: 1,
    Code.EDGE_LISTED_TWICE: 1,
    Code.BLEND_FAILED: 1,
    Code.WRONG_PART_KIND: 1,
    Code.ASSEMBLY_CYCLE: 1,
    Code.LABEL_NOT_FOUND: 1,
    Code.LABEL_NOT_OFFICIAL: 1,
    Code.INSTANCE_NOT_RELEASED: 1,
    Code.INSERT_CYCLE: 1,
    Code.INSERT_SOURCE_EMPTY: 1,
    Code.INSERT_SOURCE_FAILED: 1,
    Code.INSERT_OBSOLETE: 1,
    Code.INSERT_BEHIND: 1,
    Code.INSERT_NOT_RELEASED: 1,
    Code.BODY_DELETED: 1,
    Code.INSTANCE_FIXED: 1,
    Code.CONNECTOR_UNSUPPORTED: 1,
    Code.ASSEMBLY_UNSOLVED: 1,
    Code.ASSEMBLY_OVERCONSTRAINED: 1,
    Code.MATE_OUT_OF_LIMITS: 1,
    Code.IMPORT_FILE_MISSING: 1,
    Code.IMPORT_FILE_CHANGED: 1,
    Code.STEP_UNREADABLE: 1,
    Code.IMPORT_PRODUCT_MISSING: 1,
    Code.IMPORT_BODY_MISSING: 1,
    Code.IMPORT_EXTERNAL_MISSING: 1,
    Code.IMPORT_EXTERNAL_AMBIGUOUS: 1,
    Code.IMPORT_FAILED: 1,
    Code.EXPORT_FAILED: 1,
    Code.EXPORT_UNREPRESENTABLE: 1,
    Code.REVIEW_NOT_IN_BETA: 1,
    Code.REVIEW_INVALID_REQUEST: 1,
    Code.REVIEW_MERGE_UNAVAILABLE: 1,
    Code.REVIEW_GITHUB_UNREACHABLE: 4,
    Code.BOM_ITEM_MISSING: 1,
    Code.BOM_ITEM_DUPLICATE: 1,
    Code.BOM_ITEM_UNUSED: 1,
    Code.MEASURE_FAILED: 1,
    Code.VIEW_NOT_FOUND: 1,
    Code.APPEARANCE_UNRESOLVED: 1,
    Code.EXPLODE_NOT_FOUND: 1,
    Code.EXPLODE_UNRESOLVED: 1,
    Code.MOTION_NOT_FOUND: 1,
    Code.MOTION_UNRESOLVED: 1,
    Code.MOTION_PAST_LIMIT: 1,
    Code.MOTION_BLOCKED: 1,
    Code.MOTION_INTERFERES: 1,
    Code.LIBRARY_MISSING: 1,
    Code.LIBRARY_NESTED: 1,
    Code.LIBRARY_UNIT_INVALID: 1,
    Code.LIBRARY_PIN_INVALID: 1,
    Code.LIBRARY_GRAPHIC_INVALID: 1,
    Code.LIBRARY_OUT_OF_RANGE: 1,
    Code.LIBRARY_PAD_INVALID: 1,
    Code.LIBRARY_LAYER_UNKNOWN: 1,
    Code.LIBRARY_COURTYARD_MISSING: 1,
    Code.MODEL_FILE_MISSING: 1,
    Code.MODEL_FILE_CHANGED: 1,
    Code.MODEL_UNREADABLE: 1,
    Code.PIN_NOT_FOUND: 1,
    Code.PIN_WITHOUT_PAD: 1,
    Code.PAD_WITHOUT_PIN: 1,
    Code.PAD_MAPPED_TWICE: 1,
    Code.BLOCK_CYCLE: 1,
    Code.PORT_NOT_FOUND: 1,
    Code.NET_NOT_FOUND: 1,
    Code.NET_NAME_DUPLICATE: 1,
    Code.NET_HAS_PINS: 1,
    Code.PIN_CONFLICT: 1,
    Code.POWER_UNDRIVEN: 1,
    Code.PIN_UNCONNECTED: 1,
    Code.NC_CONNECTED: 1,
    Code.NET_SINGLE_PIN: 1,
    Code.INPUT_UNDRIVEN: 1,
    Code.REFDES_DUPLICATE: 1,
    Code.REFDES_MISSING: 1,
    Code.FOOTPRINT_MISSING: 1,
    Code.PAIR_UNMATCHED: 1,
    Code.INTENT_CONFLICT: 1,
    Code.LIBRARY_FAULT: 1,
    Code.WAIVER_UNUSED: 1,
    Code.KICAD_UNREADABLE: 1,
    Code.KICAD_LEGACY_UNSUPPORTED: 1,
    Code.IMPORT_IGNORED: 1,
    Code.IMPORT_ROUNDED: 1,
    Code.IMPORT_POWER_SKIPPED: 1,
    Code.IMPORT_DEMORGAN_DROPPED: 1,
    Code.IMPORT_LAYER_UNMAPPED: 1,
    Code.IMPORT_MODEL_SKIPPED: 1,
    Code.IMPORT_UNRESOLVED: 1,
    Code.IMPORT_VERSION_NEWER: 1,
    Code.WATCH_UNAVAILABLE: 1,
    Code.MEMORY_BUDGET_TOO_SMALL: 1,
    Code.EXPORT_MESH_FAULTS: 1,
    Code.EXPORT_TOO_LARGE: 1,
    Code.NOT_A_BOARD: 1,
    Code.ODB_UNREADABLE: 1,
    Code.ODB_VERSION_UNSUPPORTED: 1,
    Code.ODB_STEP_AMBIGUOUS: 1,
    Code.ODB_STEP_NOT_FOUND: 1,
    Code.ARCHIVE_UNSAFE: 1,
    Code.BOARD_INPUT_MISSING: 1,
    Code.BOARD_INPUT_CHANGED: 1,
    Code.ODB_LAYER_UNREADABLE: 1,
    Code.ODB_SYMBOL_UNKNOWN: 1,
    Code.ODB_SYMBOL_CYCLE: 1,
    Code.ODB_COMPONENT_INVALID: 1,
    Code.ODB_EDA_INVALID: 1,
    Code.ODB_FILE_INVALID: 1,
    Code.ODB_IGNORED: 1,
    Code.ODB_ROUNDED: 1,
    Code.ODB_ARC_INCONSISTENT: 1,
    Code.ODB_FONT_MISSING_CHAR: 1,
    Code.ODB_TEXT_DYNAMIC: 1,
    Code.ODB_BARCODE_SKIPPED: 1,
    Code.ODB_INFO_MISSING: 1,
    Code.NET_OPEN: 1,
    Code.NET_SHORT: 1,
    Code.NETLIST_POINT_OFF_COPPER: 1,
    Code.NET_CLAIM_MISMATCH: 1,
    Code.NETLIST_FROM_PINS: 1,
    Code.NETLIST_MISSING: 1,
    Code.COPPER_INCOMPLETE: 1,
    Code.COPPER_CONTACT_UNCERTAIN: 1,
    Code.CACHE_REMOTE_UNREACHABLE: 4,
    Code.CACHE_REMOTE_REFUSED: 1,
    Code.CACHE_ENTRY_BAD: 1,
    Code.WORKTREE_NOT_FOUND: 1,
    Code.WORKTREE_DIRTY: 1,
    Code.CHECKOUT_DIRTY: 1,
    Code.BRANCH_IN_WORKTREE: 1,
    Code.MERGE_IN_PROGRESS: 1,
    Code.GIT_FAILED: 1,
    Code.DEPENDENCY_INVALID: 1,
    Code.DEPENDENCY_MISSING: 1,
    Code.DEPENDENCY_NOT_FETCHED: 1,
    Code.DEPENDENCY_FETCH_FAILED: 4,
    Code.DEPENDENCY_PROJECT_MISMATCH: 1,
    Code.DEPENDENCY_REWRITTEN: 1,
    Code.DEPENDENCY_UNLOCKED: 1,
    Code.INSTANCE_FOREIGN_WORKING: 1,
    Code.INSTANCE_OUTDATED: 1,
    Code.LOCK_INVALID: 1,
    Code.LOCK_UNUSED: 1,
    Code.PROJECT_ID_DUPLICATE: 1,
    Code.LAYOUT_STALE: 1,
    Code.LAYOUT_INVALID: 1,
    Code.LAYOUT_RULE: 1,
    Code.SHEET_FULL: 1,
    Code.INSTRUCTION_CONFLICT: 1,
    Code.INSTRUCTION_UNRESOLVED: 1,
    Code.REFDES_FORMAT_INVALID: 1,
    Code.SCH_CROSSINGS: 1,
    Code.SCH_BENDS: 1,
    Code.SCH_LONG_WIRE: 1,
    Code.SCH_LABEL_WHERE_WIRE: 1,
    Code.SCH_WIRE_WHERE_LABEL: 1,
    Code.SCH_DENSITY: 1,
    Code.SCH_FLOW_REVERSED: 1,
    Code.SCH_PIN_OFF_GRID: 1,
    Code.SCH_IDIOM_MISSED: 1,
    Code.PROJECT_ID_MISSING: 1,
    Code.CANCELLED: 4,
    Code.APPROVAL_RULE_INVALID: 1,
    Code.PARAMETER_SHADOWS: 1,
    Code.PARAMETER_UNUSED: 1,
    Code.DATUM_DEGENERATE: 1,
    Code.DATUM_NO_INTERSECTION: 1,
    Code.DATUM_AMBIGUOUS: 1,
    Code.IMPORT_BUDGET_EXCEEDED: 1,
    Code.PATH_DISCONNECTED: 1,
    Code.PATH_NOT_TANGENT: 1,
    Code.PATH_SEGMENT_TWICE: 1,
    Code.PATH_BRANCHES: 1,
    Code.PATH_INVALID: 1,
    Code.HELIX_INVALID: 1,
    Code.LAYOUT_CONFLICT: 1,
    Code.SCH_POWER_SIDEWAYS: 1,
    Code.SCH_GROUND_SIDEWAYS: 1,
    Code.SCH_LABEL_VERTICAL: 1,
    Code.SCH_JOG: 1,
    Code.SCH_BYPASS_APART: 1,
    Code.SCH_PIN_NAME_OUTSIDE: 1,
    Code.SCH_PORT_BENT: 1,
    Code.SCH_FLOATING: 1,
    Code.SCH_RESISTOR_STYLE: 1,
    Code.SCH_OUT_OF_LINE: 1,
    Code.SCH_BANK_NOT_IN_ROW: 1,
    Code.SCH_TOPOLOGY: 1,
    Code.SCH_PART_FLIPPED: 1,
    Code.SCH_WIRE_WRAPS: 1,
    Code.SWEEP_FAILED: 1,
    Code.LOFT_FAILED: 1,
    Code.SHELL_FAILED: 1,
    Code.OFFSET_FAILED: 1,
    Code.MIRROR_FAILED: 1,
    Code.TWEAK_FAILED: 1,
    Code.EDIT_BUDGET_EXCEEDED: 1,
    Code.REGION_UNRESOLVED: 1,
    Code.SHEET_RESIZED: 1,
    Code.SCH_PAPER_FIT: 1,
    Code.SCH_FUNCTION_SPLIT: 1,
    Code.SCH_REGION_UNTITLED: 1,
    Code.WRONG_BODY_KIND: 1,
    Code.PARAMETER_INVALID: 1,
    Code.PART_UNIDENTIFIED: 1,
    Code.PART_UNSOURCED: 1,
    Code.PARTS_SOURCE_UNREACHABLE: 4,
    Code.PARTS_SOURCE_REFUSED: 1,
    Code.PARTS_NOT_FOUND: 1,
    Code.PARTS_CACHE_STALE: 1,
    Code.PARTS_MISMATCH: 1,
    Code.PART_OUT_OF_STOCK: 1,
    Code.PART_EXTENDED: 1,
    Code.PART_NO_DATA: 1,
    Code.PARTS_RATE_LIMITED: 4,
    Code.PART_LIFECYCLE: 1,
    Code.SHEET_METAL_FAILED: 1,
    Code.FILL_FAILED: 1,
    Code.EXTRACT_FAILED: 1,
    Code.SEW_FAILED: 1,
    Code.THICKEN_FAILED: 1,
    Code.ANNOTATION_NOT_FOUND: 1,
    Code.ANNOTATION_STALE: 1,
    Code.ANNOTATION_CLAIMED: 1,
    Code.PARTICIPANT_MODE: 1,
    Code.REFERENCE_BODY: 1,
    Code.REFERENCE_NOT_SOLID: 1,
    Code.REFERENCE_FACES_UNMESHED: 1,
    Code.REFERENCE_FACES_DROPPED: 1,
    Code.REFERENCE_FACES_SPLIT: 1,
    Code.REFERENCE_UNORIENTED: 1,
    Code.EXPORT_REFERENCE_MESH_ONLY: 1,
    Code.DRAFT_ANGLE_VARIES: 1,
    Code.THREAD_SIZE_MISMATCH: 1,
    Code.HOLE_PLACE_OFF_FACE: 1,
    Code.HOLE_PLACE_AMBIGUOUS: 1,
    Code.EXPORT_SHEET_BREP_SKIPPED: 1,
    Code.DRAWING_NOT_FOUND: 1,
    Code.DRAWING_VIEW_FAILED: 1,
    Code.DRAWING_VIEW_UNCERTAIN: 1,
    Code.DRAWING_REFERENCE_UNRESOLVED: 1,
    Code.DRAWING_REFERENCE_NOT_SHOWN: 1,
    Code.TEMPLATE_NOT_FOUND: 1,
    Code.PACKAGES_FORMAT_UNKNOWN: 1,
    Code.PACKAGE_FAILED: 1,
    Code.PACKAGE_INVALID: 1,
    Code.RELEASE_OBSOLETE: 1,
    Code.SCH_OFF_SHEET_LOCAL: 1,
    Code.SCH_BUS_UNBUNDLED: 1,
    Code.STANDARDS_INVALID: 1,
    Code.SYM_PIN_OFF_GRID: 1,
    Code.SYM_PIN_SIDE: 1,
    Code.SYM_PIN_GROUP: 1,
    Code.SYM_PIN_LENGTH: 1,
    Code.SYM_SHAPE: 1,
    Code.SYM_BODY_MARGIN: 1,
    Code.SYM_UNITS: 1,
    Code.STYLE_INVALID: 1,
    Code.STYLE_LOCKED: 1,
    Code.SCH_CONTRAST: 1,
    Code.SCH_OVERRIDE_UNUSED: 1,
    Code.ANNOTATE_CONFLICT: 1,
    Code.GITATTRIBUTES_MISSING: 1,
    Code.GIT_DRIVERS_MISSING: 1,
    Code.NON_CANONICAL: 1,
    Code.UNKNOWN_FILE: 1,
    Code.MODEL_ORPHANED: 1,
    Code.UNMERGED_PATH: 1,
    Code.GIT_HOOK_MISSING: 1,
    Code.GIT_HOOK_FOREIGN: 1,
    Code.LAYOUT_PENDING: 1,
    Code.SHEET_NOT_FOUND: 1,
    Code.REF_UNRESOLVED: 1,
    Code.REFDES_CLASH: 1,
    Code.NAME_CLASH: 1,
    Code.MERGE_FAULTS: 1,
    Code.COMPONENT_NOT_RELEASED: 1,
    Code.COMPONENT_OBSOLETE: 1,
    Code.COMPONENT_OUTDATED: 1,
    Code.LIBRARY_FOREIGN_WORKING: 1,
    Code.BOM_UNIDENTIFIED: 1,
    Code.BOARD_NOT_FOUND: 1,
    Code.BOARD_EXISTS: 1,
    Code.BOARD_DESIGN_NOT_FOUND: 1,
    Code.BOARD_OUTLINE_INVALID: 1,
    Code.BOARD_OUT_OF_RANGE: 1,
    Code.BOARD_REF_AMBIGUOUS: 1,
    Code.BOARD_FOOTPRINT_UNSUPPORTED: 1,
    Code.BOARD_TEXT_MISSING_GLYPH: 1,
    Code.BOARD_NET_NOT_FOUND: 1,
    Code.BOARD_UNPLACED: 1,
    Code.BOARD_PLACEMENT_ORPHAN: 1,
    Code.DRC_CLEARANCE: 1,
    Code.DRC_MASK_CLEARANCE: 1,
    Code.DRC_TRACK_WIDTH: 1,
    Code.DRC_DRILL: 1,
    Code.DRC_ANNULAR_RING: 1,
    Code.DRC_HOLE_SPACING: 1,
    Code.DRC_EDGE_CLEARANCE: 1,
    Code.DRC_COURTYARD_OVERLAP: 1,
    Code.DRC_COURTYARD_OPEN: 1,
    Code.SILK_REF_OVERLAP: 1,
    Code.BOARD_STACKUP_INVALID: 1,
    Code.BOARD_LAYER_NOT_FOUND: 1,
    Code.DRC_VIA_SPAN: 1,
    Code.DRC_HOLE_CLEARANCE: 1,
    Code.BOARD_CLASS_NOT_FOUND: 1,
    Code.BOARD_PRESET_NOT_FOUND: 1,
    Code.RULE_BELOW_PRESET: 1,
    Code.WAIVER_EXCEEDED: 1,
    Code.FINDING_NOT_FOUND: 1,
    Code.FINDING_NOT_WAIVABLE: 1,
    Code.PRESET_IN_USE: 1,
    Code.PLACEMENT_CONSTRAINT_UNMET: 1,
    Code.PLACEMENT_NO_ROOM: 1,
    Code.DRC_KEEPOUT: 1,
    Code.BOARD_CONTOUR_INVALID: 1,
    Code.DRC_FAILED: 1,
    Code.ITEM_NOT_FOUND: 1,
    Code.ITEM_CLASS_UNKNOWN: 1,
    Code.PART_NUMBER_UNSCHEMED: 1,
    Code.PART_NUMBER_DUPLICATE_ELSEWHERE: 1,
    Code.ITEM_LINK_DUPLICATE: 1,
    Code.ITEM_LINK_UNRESOLVED: 1,
    Code.ITEM_NUMBER_MISMATCH: 1,
    Code.BOM_UNLINKED: 1,
    Code.BOM_LINE_TWICE: 1,
    Code.BOM_CYCLE: 1,
    Code.IMPORT_CONFLICT: 1,
    Code.IMPORT_MISSING: 1,
    Code.IMPORT_MAPPING_INVALID: 1,
    Code.DOCUMENT_NOT_FOUND: 1,
    Code.DOCUMENT_BYTES_MISSING: 1,
    Code.DOCUMENT_BYTES_CHANGED: 1,
    Code.DOCUMENT_FILE_UNKNOWN: 1,
    Code.DOCUMENT_LARGE: 1,
    Code.DOCUMENT_TOO_LARGE: 1,
    Code.DOCUMENT_MISSING: 1,
    Code.DOCUMENT_UNRESOLVED: 1,
    Code.DOCUMENT_IN_USE: 1,
    Code.DOCUMENT_RELEASED: 1,
    Code.DOCUMENT_UNRELEASED: 1,
    Code.ITEM_HISTORY_LABEL_FOREIGN: 1,
    Code.PART_NUMBER_TAKEN: 1,
    Code.LEDGER_UNAVAILABLE: 4,
}
