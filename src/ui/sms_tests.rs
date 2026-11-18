//! The SMS reader and its delete path, held by tests before the code exists.
//!
//! Every test here renders the real tab through `TestBackend` or drives the real
//! write helper with a fake transport, so nothing depends on a router.

use crate::app::{App, Tab, View};
use crate::error::ApiError;
use crate::model::{DeviceExtras, SmsMessage, Snapshot};
use crate::theme::{GraphStyle, Theme};
use crate::ui::render_to_string;
use crate::write::{run_delete, run_mark_read, SmsOp, VerifyLimits};
use std::time::Duration;

fn app_with(rows: Vec<SmsMessage>) -> App {
    let snap: Snapshot =
        serde_json::from_str(include_str!("../../fixtures/sample-snapshot.json")).unwrap();
    let mut app = App::new(
        Theme::get("terminal"),
        GraphStyle::Dots,
        View::Full,
        100,
        2000,
    );
    app.on_snapshot(snap, 0);
    app.tab = Tab::Sms;
    app.on_extras(DeviceExtras {
        sms_inbox: rows,
        ..Default::default()
    });
    app
}

fn message(index: &str, body: &str) -> SmsMessage {
    SmsMessage {
        index: Some(index.to_string()),
        phone: Some("Test-Net".to_string()),
        content: Some(body.to_string()),
        date: Some("2026-01-02 03:04:05".to_string()),
        unread: true,
    }
}

fn three() -> Vec<SmsMessage> {
    vec![
        message("101", "first message body"),
        message("102", "second message body"),
        message("103", "third message body"),
    ]
}

fn long_body() -> String {
    let mut body = String::new();
    for i in 0..40 {
        body.push_str(&format!("word{i:02} "));
    }
    body.trim_end().to_string()
}

#[test]
fn cursor_moves_and_wraps_at_the_ends() {
    let mut app = app_with(three());
    assert_eq!(app.sms.cursor, 0);
    app.sms_move(1);
    assert_eq!(app.sms.cursor, 1);
    app.sms_move(-1);
    app.sms_move(-1);
    assert_eq!(app.sms.cursor, 2, "k at the top wraps to the last row");
    app.sms_move(1);
    assert_eq!(app.sms.cursor, 0, "j at the bottom wraps to the first row");
}

#[test]
fn enter_opens_the_selected_message_and_esc_closes_it() {
    let mut app = app_with(three());
    app.sms_move(1);
    assert!(app.sms_open(), "opening the cursor row must be possible");
    assert_eq!(app.sms.open.as_deref(), Some("102"));
    assert!(app.sms_close());
    assert!(app.sms.open.is_none());
}

#[test]
fn the_detail_pane_shows_the_whole_body_not_a_truncated_line() {
    let body = long_body();
    let mut app = app_with(vec![message("101", &body)]);
    app.sms_open();
    let text = render_to_string(&app, 120, 44);
    let last = body.rsplit(' ').next().expect("last word");
    assert!(
        text.contains(last),
        "the tail of the body never reached the pane:\n{text}"
    );
    assert!(
        text.contains("101") && text.contains("Test-Net"),
        "the header must carry the index and the sender:\n{text}"
    );
}

#[test]
fn long_body_wraps_on_word_boundaries_and_never_overflows_the_panel() {
    let body = long_body();
    let mut app = app_with(vec![message("101", &body)]);
    app.sms_open();
    for (w, h) in [(80u16, 30u16), (120, 40), (200, 50)] {
        let text = render_to_string(&app, w, h);
        for line in text.lines() {
            let cells = line.chars().count();
            assert!(
                cells <= w as usize,
                "a line is {cells} cells wide at {w}:\n{line}"
            );
        }
        // A word split across two rows would show a fragment; every fragment of
        // a body word must still be the whole word.
        for word in body.split(' ') {
            if !text.contains(word) {
                continue;
            }
            let mut from = 0;
            while let Some(at) = text[from..].find(word) {
                let start = from + at;
                let before = text[..start].chars().next_back();
                let after = text[start + word.len()..].chars().next();
                assert!(
                    !before.map(|c| c.is_alphanumeric()).unwrap_or(false),
                    "{word} was glued to the text before it: {:?}",
                    &text[start.saturating_sub(30)..(start + 10).min(text.len())]
                );
                assert!(
                    !after.map(|c| c.is_alphanumeric()).unwrap_or(false),
                    "{word} was cut mid-word"
                );
                from = start + word.len();
            }
        }
    }
}

#[test]
fn space_marks_and_unmarks_and_the_mark_count_is_shown() {
    let mut app = app_with(three());
    app.sms_toggle();
    app.sms_move(1);
    app.sms_toggle();
    assert_eq!(app.sms.selected.len(), 2);
    let text = render_to_string(&app, 120, 34);
    assert!(
        text.contains("2 selected"),
        "the count must be visible:\n{text}"
    );
    app.sms_toggle();
    assert_eq!(app.sms.selected.len(), 1);
}

#[test]
fn select_all_marks_every_visible_row_and_clears_on_second_press() {
    let mut app = app_with(three());
    app.sms_select_all();
    assert_eq!(app.sms.selected.len(), 3, "every row on the page");
    app.sms_select_all();
    assert!(
        app.sms.selected.is_empty(),
        "a second press clears the marks"
    );
}

#[test]
fn opening_a_message_never_posts() {
    let mut app = app_with(three());
    app.sms_move(2);
    app.sms_open();
    assert!(app.sms.open.is_some());
    assert!(
        app.sms_take_plan().is_none(),
        "opening a message must never create a write plan"
    );
    assert!(
        app.sms.selected.is_empty(),
        "opening a message must not touch the selection"
    );
}

#[test]
fn delete_requires_the_write_gate_and_a_confirmation() {
    let mut app = app_with(three());
    app.allow_write = false;
    assert!(
        app.sms_plan_delete(false).is_none(),
        "without the write gate there is no plan"
    );
    app.allow_write = true;
    let plan = app
        .sms_plan_delete(false)
        .expect("a plan once the gate is open");
    assert_eq!(plan.op, SmsOp::Delete);
    assert_eq!(plan.indices, vec!["101".to_string()]);
    assert!(
        app.sms.confirm.is_some(),
        "the confirmation must be on screen"
    );
    assert!(
        app.sms_take_plan().is_some(),
        "the plan is taken only on an explicit confirm"
    );
    assert!(app.sms.confirm.is_none(), "confirming closes the dialogue");
}

#[test]
fn confirm_defaults_to_no_and_n_cancels_without_a_post() {
    let mut app = app_with(three());
    app.allow_write = true;
    app.sms_plan_delete(false).expect("plan");
    assert!(app.sms_cancel());
    assert!(app.sms.confirm.is_none());
    assert!(
        app.sms_take_plan().is_none(),
        "a cancelled dialogue sends nothing"
    );
}

#[test]
fn a_refused_delete_is_reported_as_unsupported_with_its_code() {
    let mut app = app_with(three());
    let report = crate::write::SmsReport {
        op: SmsOp::Delete,
        attempted: 1,
        deleted: 0,
        failed: Vec::new(),
        unsupported: Some(("101".to_string(), "100003".to_string())),
        rereads: 0,
    };
    app.on_sms_report(&report);
    let blocked = app
        .sms
        .blocked
        .clone()
        .expect("the refusal must disable the keys");
    assert!(
        blocked.contains("100003"),
        "the code must be named: {blocked}"
    );
    let notice = app.sms.notice.clone().expect("a notice");
    assert!(notice.contains("unsupported"), "{notice}");
    assert!(
        app.sms_plan_delete(false).is_none(),
        "a refused path must disable the delete keys"
    );
}

#[test]
fn a_partial_bulk_delete_reports_both_counts_and_keeps_the_failed_rows() {
    let mut app = app_with(three());
    app.sms_select_all();
    let mut calls = 0;
    let report = run_delete(
        &["101".to_string(), "102".to_string(), "103".to_string()],
        |index| {
            calls += 1;
            if index == "102" {
                Err(ApiError::Router(100003))
            } else {
                Ok(())
            }
        },
        || Ok(vec!["102".to_string(), "103".to_string()]),
        VerifyLimits {
            reread: Duration::from_millis(1),
            window: Duration::from_millis(5),
        },
        |_| {},
    );
    assert_eq!(calls, 2, "the loop stops at the refusal");
    assert_eq!(report.attempted, 3);
    assert_eq!(report.deleted, 1, "only the verified one counts");
    // Two rows are named: the refused index and the one the loop never reached.
    // Silence about the second would read as "it worked".
    assert_eq!(report.failed.len(), 2, "every unhandled row is reported");
    assert!(report.unsupported.is_some());
    app.on_sms_report(&report);
    let notice = app.sms.notice.clone().expect("a notice");
    assert!(
        notice.contains("1 of 3"),
        "both counts must be named: {notice}"
    );
}

#[test]
fn delete_is_verified_by_rereading_not_assumed_from_the_post_answer() {
    let report = run_delete(
        &["101".to_string()],
        |_| Ok(()),
        || Ok(vec!["101".to_string()]),
        VerifyLimits {
            reread: Duration::from_millis(1),
            window: Duration::from_millis(3),
        },
        |_| {},
    );
    assert_eq!(report.deleted, 0, "a POST answer is not a result");
    assert_eq!(report.failed.len(), 1);
    assert!(
        report.rereads > 1,
        "the page must be read more than once, got {}",
        report.rereads
    );
    assert!(
        report.failed[0].1.contains("still present"),
        "{:?}",
        report.failed
    );
}

#[test]
fn paging_shows_range_and_total_when_the_firmware_pages() {
    let rows: Vec<SmsMessage> = (0..20)
        .map(|i| message(&format!("{}", 200 + i), "body"))
        .collect();
    let mut app = app_with(rows);
    if let Some(snap) = app.snap.as_mut() {
        if let Some(sms) = snap.sms.as_mut() {
            sms.inbox = 45;
        }
    }
    app.sms.page = 2;
    app.sms_move(-1);
    let text = render_to_string(&app, 120, 34);
    assert!(
        text.contains("21-40 of 45"),
        "the range and the total must be shown:\n{text}"
    );
}

#[test]
fn mark_read_needs_its_own_key_the_gate_and_a_reread() {
    let mut app = app_with(three());
    app.allow_write = false;
    assert!(app.sms_plan_mark_read().is_none(), "no gate, no plan");
    app.allow_write = true;
    let plan = app.sms_plan_mark_read().expect("a plan under the gate");
    assert_eq!(plan.op, SmsOp::MarkRead);
    assert_eq!(plan.indices, vec!["101".to_string()]);
    assert!(app.sms.confirm.is_some(), "marking read confirms first");

    let mut posts = 0;
    let report = run_mark_read(
        "101",
        |_| {
            posts += 1;
            Ok(())
        },
        || Ok(true),
        VerifyLimits {
            reread: Duration::from_millis(1),
            window: Duration::from_millis(3),
        },
        |_| {},
    );
    assert_eq!(posts, 1);
    assert_eq!(report.deleted, 0, "an unread reread is not a success");
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].1.contains("unread"), "{:?}", report.failed);
}

#[test]
fn no_panic_from_40x12_to_240x60() {
    let body = long_body();
    let mut app = app_with(vec![message("101", &body), message("102", &body)]);
    app.sms_open();
    for w in [40u16, 60, 80, 100, 120, 160, 240] {
        for h in [12u16, 20, 34, 60] {
            let text = render_to_string(&app, w, h);
            assert_eq!(text.lines().count(), h as usize);
        }
    }
}

#[test]
fn a_delete_plan_is_drawn_as_a_modal_with_both_answers() {
    let mut app = app_with(three());
    app.allow_write = true;
    app.sms_toggle();
    app.sms_move(1);
    app.sms_toggle();
    app.sms_plan_delete(false).expect("a plan under the gate");
    let text = render_to_string(&app, 120, 34);
    assert!(text.contains("delete messages?"), "{text}");
    assert!(text.contains("delete 2 messages"), "{text}");
    assert!(
        text.contains("index 101") && text.contains("index 102"),
        "{text}"
    );
    assert!(
        text.contains("a deleted message cannot be recovered"),
        "the warning must be on screen"
    );
    assert!(
        text.contains("y confirm") && text.contains("n / Esc cancel"),
        "both keys must be named"
    );

    app.sms_cancel();
    let after = render_to_string(&app, 120, 34);
    assert!(
        !after.contains("delete messages?"),
        "the modal must go when the dialogue is cancelled"
    );
}

#[test]
fn a_long_plan_is_cut_after_eight_indexes_and_says_how_many_more() {
    let rows: Vec<_> = (1..=11)
        .map(|i| message(&format!("{}", 200 + i), "body"))
        .collect();
    let mut app = app_with(rows);
    app.allow_write = true;
    app.sms_select_all();
    app.sms_plan_delete(false).expect("a plan under the gate");
    let text = render_to_string(&app, 120, 34);
    assert!(text.contains("delete 11 messages"), "{text}");
    assert!(text.contains("and 3 more"), "{text}");
    assert_eq!(
        text.matches("  index ").count(),
        8,
        "the list stops at eight rows"
    );
}

#[test]
fn the_mark_read_plan_has_its_own_title_and_no_delete_warning() {
    let mut app = app_with(three());
    app.allow_write = true;
    app.sms_open();
    app.sms_plan_mark_read().expect("a plan under the gate");
    let text = render_to_string(&app, 120, 34);
    assert!(text.contains("mark read?"), "{text}");
    assert!(text.contains("mark read 1 message (index 101)"), "{text}");
    assert!(
        !text.contains("cannot be recovered"),
        "marking read is reversible; the delete warning must not appear"
    );
    assert!(text.contains("y confirm"), "{text}");
}

#[test]
fn the_modal_never_overflows_a_small_frame() {
    let mut app = app_with(three());
    app.allow_write = true;
    app.sms_select_all();
    app.sms_plan_delete(false).expect("plan");
    for w in [40u16, 60, 80] {
        for h in [12u16, 16, 20] {
            let text = render_to_string(&app, w, h);
            assert_eq!(text.lines().count(), h as usize, "{w}x{h}");
        }
    }
}
