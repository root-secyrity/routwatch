# Design contracts

Rules this tool promises to keep, and the test that fails when one breaks.
Nothing here is a feature description: each row is a behaviour a future change
could quietly undo, so it is listed with its guard.

This table is checked by `src/contracts.rs`: every row must name a file in this
repository and a test function that exists in it. Renaming a test therefore
breaks `cargo test` here rather than leaving the promise unattended.

| Contract | Guarded by | File |
| --- | --- | --- |
| A confirming write never hides the outage it may cause | `the_preflight_never_hides_the_radio_restart` | `src/preflight.rs` |
| A write is shown with both sides before it is applied | `the_confirm_modal_shows_the_after_state_and_the_outage` | `src/ui/mod.rs` |
| A report never ships identity fields by default | `the_report_hides_identity_but_keeps_what_a_helper_needs` | `src/report.rs` |
| A report file is private and never overwrites another | `the_report_file_is_private_and_never_overwrites` | `src/report.rs` |
| A small terminal is told the numbers it needs | `the_too_small_screen_names_the_numbers_it_needs` | `src/ui/mod.rs` |
| Every size the frame claims to support renders | `the_frame_survives_the_sizes_it_claims_to_support` | `src/ui/mod.rs` |
| The header names the session state, not only its age | `the_header_names_the_session_state_not_only_the_age` | `src/ui/mod.rs` |
| The feed keeps your place while new lines arrive | `the_feed_keeps_the_selected_line_when_new_events_arrive` | `src/app.rs` |
| An unpaused feed follows the newest line, a paused one holds | `a_running_feed_stays_on_the_newest_line` | `src/app.rs` |
| The feed never draws past forty columns | `the_feed_fits_a_forty_column_terminal` | `src/ui/mod.rs` |
| A metric that was not measured is not drawn | `a_missing_conditional_tile_leaves_no_hole` | `src/ui/tiles.rs` |
| No palette takes the magenta slot | `no_derived_palette_takes_the_magenta_slot` | `src/theme.rs` |
| A derived palette keeps body text readable | `every_derived_palette_keeps_text_readable` | `src/theme.rs` |
| Machine output stays flat and free of secrets | `status_json_is_flat_and_secret_free` | `src/headless.rs` |
| A snapshot without a sensor omits the temperature key | `a_snapshot_without_a_sensor_omits_the_temperature_key` | `src/headless.rs` |
| A diagnostic names only the credential sources it consults | `the_credential_message_names_only_rungs_the_lookup_uses` | `src/config.rs` |
| A piped password is a credential, not a keyword `doctor` ignores | `doctor_credits_the_stdin_password_instead_of_denying_it` | `src/headless.rs` |
| A diagnostic asks the path every firmware actually serves | `doctor_reaches_the_mock_and_keeps_probing` | `src/mock_cpe.rs` |
| A list the firmware hides is named, never drawn as zero | `a_hidden_client_list_is_recorded_not_zeroed` | `src/mock_cpe.rs` |
| A redacted dump keeps every field name and no identity value | `identity_values_are_replaced_and_every_field_name_survives` | `src/scrub.rs` |
| The ask-for-a-model document carries no identity | `the_shared_document_has_no_identity_and_keeps_the_shapes` | `src/share.rs` |

## Adding a contract

1. Write the test first and watch it fail for the right reason.
2. Add one row above: the promise in plain words, the test name verbatim, the
   file the test lives in.
3. Run `cargo test --locked contracts` - a typo in the test name fails there.
