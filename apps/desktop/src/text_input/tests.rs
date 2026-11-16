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
fn masked_text_keeps_offsets() {
    let text = "s3cret-é";
    let masked = masked_text(text);
    assert_eq!(masked.len(), text.len());
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
                let last_grapheme = text.grapheme_indices(true).last().unwrap().0;
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
        let position = cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                let layout = input.last_layout.as_ref().unwrap();
                // The first '*' inside this grapheme used to produce an invalid byte offset.
                let position = layout.origin()
                    + layout.position_for_offset(prefix.len() + 1)
                    + point(px(0.1), px(12.));
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
