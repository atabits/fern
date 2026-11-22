use super::*;
use gpui::{Modifiers, TestAppContext, VisualTestContext};

struct InputHost {
    input: Entity<TextInput>,
}

impl Render for InputHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Deliberately no parent overflow clipping: the input owns its viewport.
        div()
            .flex()
            .items_start()
            .w_full()
            .p(px(12.))
            .text_size(px(16.))
            .line_height(px(24.))
            .child(self.input.clone())
    }
}

fn setup(cx: &mut TestAppContext, mode: Mode) -> (Entity<TextInput>, &mut VisualTestContext) {
    cx.update(theme::init);
    let (host, cx) = cx.add_window_view(|window, cx| {
        let input = cx.new(|cx| TextInput::new(mode, "A placeholder", cx));
        window.focus(&input.focus_handle(cx), cx);
        InputHost { input }
    });
    cx.simulate_resize(size(px(200.), px(2000.)));
    cx.run_until_parked();
    let input = cx.update(|_, cx| host.read(cx).input.clone());
    (input, cx)
}

fn assert_caret_visible(input: &Entity<TextInput>, cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            let layout = input.last_layout.as_ref().unwrap();
            let bounds = layout.bounds;
            let offset = input.offset_to_utf16(input.cursor_offset());
            let caret = input
                .bounds_for_range(offset..offset, bounds, window, cx)
                .unwrap();
            assert!(
                caret.left() >= bounds.left() - px(0.01),
                "{caret:?} outside {bounds:?}"
            );
            assert!(
                caret.right() + CARET_WIDTH <= bounds.right() + px(0.01),
                "{caret:?} outside {bounds:?}"
            );
            assert!(caret.top() >= bounds.top());
            assert!(caret.bottom() <= bounds.bottom() + px(0.01));
        });
    });
}

fn assert_paint_is_clipped(input: &Entity<TextInput>, cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let bounds = input
            .read(cx)
            .last_layout
            .as_ref()
            .unwrap()
            .bounds
            .scale(window.scale_factor());
        let quads = window.painted_quads();
        assert!(!quads.is_empty());
        for quad in quads {
            assert_eq!(quad.content_mask.bounds, bounds);
        }
        for underline in window.painted_underlines() {
            assert_eq!(underline.content_mask.bounds, bounds);
        }
    });
}

#[test]
fn masked_text_counts_unicode_chars_not_utf8_bytes() {
    let text = "s3cret-é漢🙂";
    let masked = masked_text(text);
    assert_eq!(masked.len(), text.chars().count());
    assert_eq!(masked.len(), 10);
    assert!(masked.chars().all(|c| c == '*'));
}

#[gpui::test]
fn long_single_line_text_scrolls_without_losing_content_or_selection(cx: &mut TestAppContext) {
    let (input, cx) = setup(cx, Mode::SingleLine);
    for text in [
        "A long task title with spaces ".repeat(30),
        "x".repeat(400),
        "漢字仮名".repeat(100),
        "👩‍💻🙂e\u{301}".repeat(60),
    ] {
        cx.update(|window, cx| input.update(cx, |input, cx| input.clear(window, cx)));
        cx.simulate_input(&text);
        assert_caret_visible(&input, cx);
        assert_paint_is_clipped(&input, cx);
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                assert_eq!(input.text(), text);
                assert!(input.last_layout.as_ref().unwrap().scroll_left > px(0.));
                input.home(&Home, window, cx);
            })
        });
        assert_caret_visible(&input, cx);
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                assert_eq!(input.selection(), 0..0);
                assert_eq!(input.last_layout.as_ref().unwrap().scroll_left, px(0.));
                input.end(&End, window, cx);
            })
        });
        assert_caret_visible(&input, cx);
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                assert_eq!(input.selection(), text.len()..text.len());
                input.select_to(0, window, cx);
            })
        });
        assert_caret_visible(&input, cx);
        assert_paint_is_clipped(&input, cx);
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                assert!(input.selection_reversed);
                input.copy(&Copy, window, cx);
                assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), text);
                input.cut(&Cut, window, cx);
                assert!(input.text().is_empty());
                input.paste(&Paste, window, cx);
                assert_eq!(input.text(), text);
            })
        });
        assert_caret_visible(&input, cx);
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.left(&Left, window, cx);
                let last_grapheme = text.grapheme_indices(true).next_back().unwrap().0;
                assert_eq!(input.selection(), last_grapheme..last_grapheme);
            })
        });
        assert_caret_visible(&input, cx);
    }
}

#[gpui::test]
fn scrolled_input_mouse_and_ime_use_the_visible_text_coordinates(cx: &mut TestAppContext) {
    let (input, cx) = setup(cx, Mode::SingleLine);
    let text = "A task with an increasingly long title ".repeat(20) + "漢字🙂abc";
    cx.simulate_input(&text);
    assert_caret_visible(&input, cx);
    let offset = text.len() - 2;
    let position = cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            let layout = input.last_layout.as_ref().unwrap();
            let position =
                layout.origin() + layout.position_for_offset(offset) + point(px(0.1), px(12.));
            assert_eq!(input.index_for_mouse_position(position), offset);
            assert_eq!(
                input.character_index_for_point(position, window, cx),
                Some(input.offset_to_utf16(offset))
            );
            position
        })
    });
    cx.simulate_click(position, Modifiers::default());
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            assert_eq!(input.selection(), offset..offset);
            input.end(&End, window, cx);
            input.replace_and_mark_text_in_range(None, "日本🙂", Some(4..4), window, cx);
            assert_eq!(
                input.marked_text_range(window, cx),
                Some(text.encode_utf16().count()..text.encode_utf16().count() + 4)
            );
        })
    });
    assert_caret_visible(&input, cx);
    assert_paint_is_clipped(&input, cx);
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.replace_text_in_range(None, "日本語", window, cx);
            assert_eq!(input.text(), text + "日本語");
            assert!(input.marked_text_range(window, cx).is_none());
        })
    });
    assert_caret_visible(&input, cx);
}

#[gpui::test]
fn input_viewport_tracks_resize_and_returns_to_the_start_after_clear(cx: &mut TestAppContext) {
    let (input, cx) = setup(cx, Mode::SingleLine);
    cx.simulate_input(&"a".repeat(50));
    assert_caret_visible(&input, cx);
    for width in [120., 700., 150.] {
        cx.simulate_resize(size(px(width), px(2000.)));
        assert_caret_visible(&input, cx);
        assert_paint_is_clipped(&input, cx);
        cx.update(|_, cx| {
            let scroll = input.read(cx).last_layout.as_ref().unwrap().scroll_left;
            if width == 700. {
                assert_eq!(scroll, px(0.));
            } else {
                assert!(scroll > px(0.));
            }
        });
    }
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.clear(window, cx);
            input.set_placeholder("A placeholder longer than the field can display", cx);
        })
    });
    assert_caret_visible(&input, cx);
    assert_paint_is_clipped(&input, cx);
    cx.update(|_, cx| {
        assert_eq!(
            input.read(cx).last_layout.as_ref().unwrap().scroll_left,
            px(0.)
        );
    });
}

#[gpui::test]
fn long_tokens_wrap_and_keep_the_caret_inside_multiline_inputs(cx: &mut TestAppContext) {
    for mode in [Mode::Wrapped, Mode::MultiLine] {
        let (input, cx) = setup(cx, mode);
        for token in ["abcdefghijklmno", "漢字仮名交じり文", "👩‍💻🙂e\u{301}"] {
            let text = if mode == Mode::MultiLine {
                token.repeat(12) + "\n" + &token.repeat(8)
            } else {
                token.repeat(20)
            };
            cx.update(|_, cx| input.update(cx, |input, cx| input.set_text(text.clone(), cx)));
            cx.simulate_resize(size(px(200.), px(2000.)));
            assert_caret_visible(&input, cx);
            assert_paint_is_clipped(&input, cx);
            let narrow_height = cx.update(|_, cx| {
                let input = input.read(cx);
                assert_eq!(input.text(), text);
                let layout = input.last_layout.as_ref().unwrap();
                assert_eq!(layout.scroll_left, px(0.));
                assert!(layout.bounds.size.height > layout.line_height);
                for (offset, _) in text.grapheme_indices(true) {
                    assert!(
                        layout.position_for_offset(offset).x + CARET_WIDTH
                            <= layout.bounds.size.width + px(0.01)
                    );
                }
                layout.bounds.size.height
            });
            cx.simulate_resize(size(px(500.), px(2000.)));
            assert_caret_visible(&input, cx);
            cx.update(|_, cx| {
                let layout = input.read(cx).last_layout.as_ref().unwrap();
                assert!(
                    layout.bounds.size.height < narrow_height,
                    "{mode:?} {token:?}: {narrow_height:?} -> {:?}, width {:?}",
                    layout.bounds.size.height,
                    layout.bounds.size.width
                );
            });
        }
    }
}

#[gpui::test]
#[allow(clippy::single_range_in_vec_init)]
fn recognised_text_highlights_and_selection_share_the_input_clip(cx: &mut TestAppContext) {
    let (input, cx) = setup(cx, Mode::SingleLine);
    let text = "tomorrow ".repeat(40);
    cx.simulate_input(&text);
    cx.update(|_, cx| {
        input.update(cx, |input, cx| {
            input.set_highlights(vec![0..text.len()], cx);
        })
    });
    assert_paint_is_clipped(&input, cx);
    cx.update(|window, cx| input.update(cx, |input, cx| input.select_all(&SelectAll, window, cx)));
    assert_paint_is_clipped(&input, cx);
}

#[gpui::test]
fn scrolled_masked_text_hit_testing_keeps_unicode_graphemes_intact(cx: &mut TestAppContext) {
    let (input, cx) = setup(cx, Mode::SingleLine);
    let prefix = "password".repeat(30);
    for suffix in ["🙂", "漢", "e\u{301}", "👩‍💻"] {
        let text = prefix.clone() + suffix;
        cx.update(|_, cx| {
            input.update(cx, |input, cx| {
                input.masked = true;
                input.set_text(text.clone(), cx);
            })
        });
        assert_caret_visible(&input, cx);
        let suffix_chars = suffix.chars().count();
        let position = cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                let layout = input.last_layout.as_ref().unwrap();
                assert_eq!(layout.paragraphs[0].line.len(), text.chars().count());
                // For multi-scalar graphemes (e.g. e + combining acute, ZWJ emoji), clicking on
                // the inner '*' must snap to a grapheme boundary; for single-scalar glyphs, click
                // on the left edge of its single '*'.
                let display_idx = if suffix_chars > 1 { prefix.len() + 1 } else { prefix.len() };
                let position = layout.origin()
                    + layout.position_for_offset(display_idx)
                    + point(px(0.), px(12.));
                assert_eq!(input.index_for_mouse_position(position), prefix.len());
                assert_eq!(
                    input.character_index_for_point(position, window, cx),
                    Some(prefix.len())
                );
                position
            })
        });
        cx.simulate_click(position, Modifiers::default());
        cx.simulate_input("x");
        cx.update(|_, cx| assert_eq!(input.read(cx).text(), prefix.clone() + "x" + suffix));
        assert_caret_visible(&input, cx);
    }
}

#[test]
fn markdown_list_continuation_handles_bullets_checklists_quotes_and_numbers() {
    assert_eq!(markdown_list_continuation("- [ ] Buy milk"), Some(ListEnter::Continue("- [ ] ".into())));
    assert_eq!(markdown_list_continuation("- [x] Done item"), Some(ListEnter::Continue("- [ ] ".into())));
    assert_eq!(markdown_list_continuation("- [ ] "), Some(ListEnter::ClearLine));
    assert_eq!(markdown_list_continuation("  - Nested"), Some(ListEnter::Continue("  - ".into())));
    assert_eq!(markdown_list_continuation("* Bullet"), Some(ListEnter::Continue("* ".into())));
    assert_eq!(markdown_list_continuation("- "), Some(ListEnter::ClearLine));
    assert_eq!(markdown_list_continuation("> Quote"), Some(ListEnter::Continue("> ".into())));
    assert_eq!(markdown_list_continuation("1. First"), Some(ListEnter::Continue("2. ".into())));
    assert_eq!(markdown_list_continuation("9. "), Some(ListEnter::ClearLine));
    assert_eq!(markdown_list_continuation("Plain text"), None);
}

#[test]
fn adjust_lines_indent_indents_and_outdents_single_and_multiple_lines() {
    let (out, sel) = adjust_lines_indent("- item", 4..4, true);
    assert_eq!(out, "  - item");
    assert_eq!(sel, 6..6);

    let (out, sel) = adjust_lines_indent("  - item", 6..6, false);
    assert_eq!(out, "- item");
    assert_eq!(sel, 4..4);

    let (out, sel) = adjust_lines_indent("- a\n- b", 1..6, true);
    assert_eq!(out, "  - a\n  - b");
    assert_eq!(sel, 3..10);

    let (out, sel) = adjust_lines_indent("  a\n  b", 2..7, false);
    assert_eq!(out, "a\nb");
    assert_eq!(sel, 0..3);

    // Trailing newline at end of selection should not indent the next untouched line.
    let (out, sel) = adjust_lines_indent("a\nb", 0..2, true);
    assert_eq!(out, "  a\nb");
    assert_eq!(sel, 2..4);
}

#[gpui::test]
#[allow(clippy::single_range_in_vec_init)]
fn highlights_are_normalized_and_cleared_on_edits_undo_and_redo(cx: &mut TestAppContext) {
    let (input, cx) = setup(cx, Mode::SingleLine);
    // Multi-byte UTF-8: "Привет мир"
    let text = "Привет мир";
    cx.simulate_input(text);

    // Unsorted, overlapping, mid-char-boundary, inverted, and out-of-bounds ranges.
    cx.update(|_, cx| {
        input.update(cx, |input, cx| {
            #[allow(clippy::reversed_empty_ranges)]
            let ranges = vec![10..5, 13..999, 1..5, 3..8];
            input.set_highlights(ranges, cx);
            // Every stored range must be strictly increasing, non-overlapping, on char boundaries, and in-bounds.
            let mut prev_end = 0;
            for r in &input.highlights {
                assert!(r.start < r.end);
                assert!(r.start >= prev_end);
                assert!(r.end <= input.text().len());
                assert!(input.text().is_char_boundary(r.start));
                assert!(input.text().is_char_boundary(r.end));
                prev_end = r.end;
            }
            assert!(!input.highlights.is_empty());
        });
    });
    cx.run_until_parked();

    // replace_text_in_range clears highlights so shortening text cannot panic display_runs().
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.select_all(&SelectAll, window, cx);
            input.replace_text_in_range(None, "x", window, cx);
            assert!(input.highlights.is_empty());
        });
    });
    cx.run_until_parked();

    // edit() clears highlights.
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.set_highlights(Vec::from([0..1]), cx);
            input.edit("ab".into(), 2..2, window, cx);
            assert!(input.highlights.is_empty());
        });
    });
    cx.run_until_parked();

    // undo() and redo() clear highlights.
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.set_highlights(Vec::from([0..2]), cx);
            input.undo(&Undo, window, cx);
            assert!(input.highlights.is_empty());
            input.set_highlights(Vec::from([0..1]), cx);
            input.redo(&Redo, window, cx);
            assert!(input.highlights.is_empty());
        });
    });
    cx.run_until_parked();
}

#[gpui::test]
#[allow(clippy::reversed_empty_ranges)]
fn inverted_utf16_ranges_do_not_panic(cx: &mut TestAppContext) {
    let (input, cx) = setup(cx, Mode::SingleLine);
    cx.simulate_input("hello🙂world");
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            let mut actual = None;
            let got = input.text_for_range(8..2, &mut actual, window, cx);
            assert_eq!(got.as_deref(), Some("llo🙂w"));
            assert_eq!(actual, Some(2..8));

            input.set_selected_text_range(7..1, window, cx);
            assert!(input.selection().start <= input.selection().end);

            input.replace_text_in_range(Some(7..2), "X", window, cx);
            assert_eq!(input.text(), "heXworld");
        });
    });
    cx.run_until_parked();
}

#[gpui::test]
fn paste_normalizes_crlf_and_cr_line_endings(cx: &mut TestAppContext) {
    let (multi, cx) = setup(cx, Mode::MultiLine);
    cx.update(|window, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("line1\r\nline2\rline3".into()));
        multi.update(cx, |input, cx| input.paste(&Paste, window, cx));
        assert_eq!(multi.read(cx).text(), "line1\nline2\nline3");
        assert!(!multi.read(cx).text().contains('\r'));
    });
    cx.run_until_parked();

    let (single, cx) = setup(cx, Mode::SingleLine);
    cx.update(|window, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("foo\r\nbar\rbaz".into()));
        single.update(cx, |input, cx| input.paste(&Paste, window, cx));
        assert_eq!(single.read(cx).text(), "foo bar baz");
        assert!(!single.read(cx).text().contains('\r'));
    });
    cx.run_until_parked();
}

#[gpui::test]
fn mouse_click_mid_ime_clears_marked_range(cx: &mut TestAppContext) {
    let (input, cx) = setup(cx, Mode::SingleLine);
    cx.simulate_input("hello ");
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "nihao", None, window, cx);
            assert!(input.marked_text_range(window, cx).is_some());
        });
    });
    cx.run_until_parked();

    let start_pos = cx.update(|_, cx| {
        let layout = input.read(cx).last_layout.as_ref().unwrap();
        layout.origin() + layout.position_for_offset(0) + point(px(1.), px(12.))
    });
    cx.simulate_click(start_pos, Modifiers::default());
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            assert!(input.marked_text_range(window, cx).is_none());
        });
    });
    cx.simulate_input("!");
    cx.update(|_, cx| {
        assert_eq!(input.read(cx).text(), "!hello nihao");
    });
}

#[gpui::test]
fn ime_preedit_does_not_flood_undo_stack(cx: &mut TestAppContext) {
    let (input, cx) = setup(cx, Mode::SingleLine);
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.set_text("hello ", cx);
            assert!(input.undo_stack.is_empty());

            // Simulate multi-step IME composition: n -> ni -> nih -> nihao -> commit "你好".
            for preedit in ["n", "ni", "nih", "nihao"] {
                input.replace_and_mark_text_in_range(None, preedit, None, window, cx);
            }
            assert_eq!(input.undo_stack.len(), 1);
            input.replace_text_in_range(None, "你好", window, cx);
            assert_eq!(input.text(), "hello 你好");
            assert_eq!(input.undo_stack.len(), 1);

            // A single Undo returns cleanly to pre-composition state.
            input.undo(&Undo, window, cx);
            assert_eq!(input.text(), "hello ");
            input.redo(&Redo, window, cx);
            assert_eq!(input.text(), "hello 你好");

            // Cancelling a composition back to empty does not leave a no-op undo entry.
            input.replace_and_mark_text_in_range(None, "a", None, window, cx);
            input.replace_and_mark_text_in_range(None, "", None, window, cx);
            assert_eq!(input.text(), "hello 你好");
            assert_eq!(input.undo_stack.len(), 1);
        });
    });
}

#[gpui::test]
fn enter_after_list_prefix_with_trailing_text_inserts_newline(cx: &mut TestAppContext) {
    let (input, cx) = setup(cx, Mode::MultiLine);
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.set_text("- [ ] hello", cx);
            // Place caret right after "- [ ] " (before "hello").
            input.move_to(6, window, cx);
            input.enter(&Enter, window, cx);
            assert_eq!(input.text(), "- [ ] \nhello");

            // On a truly empty list prefix line, Enter still clears the prefix.
            input.set_text("- [ ] ", cx);
            input.enter(&Enter, window, cx);
            assert_eq!(input.text(), "");
        });
    });
}

#[gpui::test]
fn wire_dialog_input_handles_submit_and_cancel(cx: &mut TestAppContext) {
    struct DialogHost {
        input: Entity<TextInput>,
        submitted: Option<String>,
        cancelled: bool,
        _subs: [Subscription; 2],
    }

    impl Render for DialogHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().child(self.input.clone())
        }
    }

    cx.update(theme::init);
    let (host, cx) = cx.add_window_view(|window, cx| {
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, "Name", cx));
        let subs = wire_dialog_input(
            &input,
            cx,
            |this: &mut DialogHost, text, _| this.submitted = Some(text.to_owned()),
            |this: &mut DialogHost, _| this.cancelled = true,
        );
        window.focus(&input.focus_handle(cx), cx);
        DialogHost { input, submitted: None, cancelled: false, _subs: subs }
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        let input = host.read(cx).input.clone();
        input.update(cx, |input, cx| {
            input.set_text("Project X", cx);
            input.enter(&Enter, window, cx);
            input.cancel(&Cancel, window, cx);
        });
    });
    cx.update(|_, cx| {
        let dialog = host.read(cx);
        assert_eq!(dialog.submitted.as_deref(), Some("Project X"));
        assert!(dialog.cancelled);
    });
}

#[gpui::test]
fn read_only_blocks_edits_while_allowing_selection_and_copy(cx: &mut TestAppContext) {
    let (input, cx) = setup(cx, Mode::MultiLine);
    cx.update(|_, cx| {
        input.update(cx, |input, cx| {
            input.set_text("Immutable record", cx);
            input.set_read_only(true, cx);
        });
    });

    cx.simulate_input(" edited");
    cx.update(|window, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("pasted".into()));
        input.update(cx, |input, cx| {
            input.backspace(&Backspace, window, cx);
            input.delete(&Delete, window, cx);
            input.paste(&Paste, window, cx);
            input.enter(&Enter, window, cx);
            input.edit("replaced".into(), 0..8, window, cx);
            input.select_all(&SelectAll, window, cx);
            input.cut(&Cut, window, cx);
            assert_eq!(input.text(), "Immutable record");
            input.copy(&Copy, window, cx);
        });
    });
    cx.update(|_, cx| {
        assert_eq!(input.read(cx).text(), "Immutable record");
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("Immutable record")
        );
    });
}
