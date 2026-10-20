use super::*;
use wordcraft_doc::props::{BorderStyle, LineSpacing, VAlign};
use wordcraft_doc::{Block, Rgb};

fn paragraph(flow: &Flow, index: usize) -> &Para {
    match &flow.blocks[index] {
        FBlock::Para(p) => p,
        _ => panic!("expected paragraph"),
    }
}

fn table(flow: &Flow) -> &FTable {
    match &flow.blocks[0] {
        FBlock::Table(t) => t,
        _ => panic!("expected table"),
    }
}

fn run(para: &Para) -> &Fmt {
    match &para.inlines[0] {
        Inline::Text(_, fmt) => fmt,
        _ => panic!("expected text"),
    }
}

#[test]
fn explicit_font_families_keep_the_first_family_without_monospace_substitution() {
    for (html, expected) in [
        (r#"<p style="font-family:'Golden Paste Mono','Courier New',monospace">Text</p>"#, "Golden Paste Mono"),
        (r#"<p style="font-family:'Courier New','Golden Paste Mono',monospace">Text</p>"#, "Courier New"),
        (r#"<p><font face="'Golden Paste Mono','Courier New',monospace">Text</font></p>"#, "Golden Paste Mono"),
        (r#"<p><font face="Courier New,Golden Paste Mono,monospace">Text</font></p>"#, "Courier New"),
        (r#"<p><code><font face="Golden Paste Mono,Courier New,monospace">Text</font></code></p>"#, "Golden Paste Mono"),
    ] {
        let flow = parse(html);
        let fmt = run(paragraph(&flow, 0));
        assert_eq!(fmt.font.as_deref(), Some(expected), "{html}");
        assert!(!fmt.code, "{html}");
        let doc = model::to_doc(&flow);
        let Block::Para(p) = &*doc.body[0] else { panic!("expected paragraph") };
        assert_eq!(p.runs[0].props.font.as_deref(), Some(expected), "{html}");
    }
}

#[test]
fn underlined_links_keep_semantic_underline_on_export() {
    for (content, underline) in [("<u>Text</u>", true), ("Text", false)] {
        let flow = parse(&format!(r#"<p><a href="https://example.test/">{content}</a></p>"#));
        let html = export_fragment(&model::to_doc(&flow));
        assert!(html.contains(r#"<a href="https://example.test/">"#), "{html}");
        assert_eq!(html.contains("<u>Text</u>"), underline, "{html}");
    }
}

#[test]
fn office_stylesheets_cascade_with_inline_precedence() {
    let flow = parse(r#"
        <style>
          @import url('https://invalid.test/external.css');
          @media screen { p { color: yellow; } }
          p.note { color: #123456; font-style: italic; }
          .note { color: blue; font-weight: 600; }
          p { color: red; font-size: 10pt; }
          .note { font-size: 11pt; }
          p:hover, #bad, p span { font-size: 99pt; }
        </style>
        <p class='note extra' style='color: #abcdef'>First</p>
        <p class='note'>Second</p>
        <p>Third</p>
        <style>p.note { text-decoration: underline; }</style>
    "#);
    assert_eq!(flow.blocks.len(), 3);
    let first = run(paragraph(&flow, 0));
    assert_eq!(first.color, Some(Rgb(0xab, 0xcd, 0xef)));
    assert_eq!(first.size, Some(11.0));
    assert!(first.bold && first.italic && first.underline);
    assert_eq!(run(paragraph(&flow, 1)).color, Some(Rgb(0x12, 0x34, 0x56)));
    assert_eq!(run(paragraph(&flow, 2)).color, Some(Rgb(255, 0, 0)));
    let flow = parse("<style>@import url('https://invalid.test/x'); p {color:red}</style><p>Kept</p>");
    assert_eq!(run(paragraph(&flow, 0)).color, Some(Rgb(255, 0, 0)));
}

#[test]
fn word_paragraph_indents_spacing_and_line_height_reach_document() {
    let flow = parse(r#"<style>p.MsoNormal { font-size:16pt; margin-left:1in;
      margin-right:24px; text-indent:-.5in; margin-top:1cm; margin-bottom:2em;
      line-height:150%; text-align:justify; }</style>
      <p class='MsoNormal'>Indented</p><p style='line-height:20px'>Exact</p>
      <p style='line-height:1.25'>Multiple</p>"#);
    let p = paragraph(&flow, 0);
    assert_eq!(p.spacing.left, Some(72.0));
    assert_eq!(p.spacing.right, Some(18.0));
    assert_eq!(p.spacing.first, Some(-36.0));
    assert!((p.spacing.before.unwrap() - 72.0 / 2.54).abs() < 0.001);
    assert_eq!(p.spacing.after, Some(32.0));
    assert_eq!(p.spacing.line, Some(LineSpacing::Multiple(1.5)));
    assert_eq!(p.align, Some(Align::Justify));
    assert_eq!(paragraph(&flow, 1).spacing.line, Some(LineSpacing::Exactly(15.0)));
    assert_eq!(paragraph(&flow, 2).spacing.line, Some(LineSpacing::Multiple(1.25)));
    let doc = model::to_doc(&flow);
    let Block::Para(p) = &*doc.body[0] else { panic!("expected paragraph") };
    assert_eq!(p.props.indent_left, Some(72.0));
    assert_eq!(p.props.indent_first, Some(-36.0));
    assert_eq!(p.props.space_after, Some(32.0));
    assert_eq!(p.props.line_spacing, Some(LineSpacing::Multiple(1.5)));
}

#[test]
fn word_mso_lists_discard_support_markers_and_create_numbering() {
    let flow = parse(r#"<style>p.MsoListParagraph { mso-list:l0 level1 lfo1; }</style>
      <p class='MsoListParagraph'><![if !supportLists]><span>&#183;&nbsp;</span><![endif]>Bullet</p>
      <p style='mso-list:l0 level2 lfo1'><span style='mso-list:Ignore'>1.<span>&nbsp;</span></span>Nested</p>
      <p style='mso-list:l0 level1 lfo1'><!--[if !supportLists]><span>2.&nbsp;</span><![endif]-->Number</p>"#);
    for (i, text, ordered, level) in [(0, "Bullet", false, 0), (1, "Nested", true, 1), (2, "Number", true, 0)] {
        let p = paragraph(&flow, i);
        assert_eq!(p.text(), text);
        assert_eq!(p.list, Some(ListInfo { ordered, level }));
    }
    let doc = model::to_doc(&flow);
    assert!(doc.body.iter().all(|b| matches!(&**b, Block::Para(p) if p.props.numbering.is_some())));
}

#[test]
fn google_docs_flat_list_margins_preserve_nesting() {
    let flow = parse(r#"<ul style='margin-left:0pt'><li style='margin-left:36pt'>Outer</li>
      <li style='margin-left:72pt'>Inner</li><li style='margin-left:36pt'>Outer again</li></ul>"#);
    assert_eq!(flow.blocks.len(), 3);
    for (i, level) in [(0, 0), (1, 1), (2, 0)] {
        assert_eq!(paragraph(&flow, i).list, Some(ListInfo { ordered: false, level }));
    }
}

#[test]
fn libreoffice_table_cells_keep_borders_shading_alignment_and_column_widths() {
    let flow = parse(r#"<style>td.office { border:2pt dashed #123456; border-top:none;
      background-color:rgb(240, 241, 242); vertical-align:middle; }</style>
      <!--[if gte mso 9]><xml>discarded</xml><![endif]-->
      <table><col width='120'><col style='width:2in'><col width='80'>
      <tr><td class='office' rowspan='2'>A<o:p>discarded</o:p></td>
      <td colspan='2' style='border:none;width:204pt'>B</td></tr>
      <tr><td width='192' style='border-bottom:2px double rgb(1, 2, 3)'>C</td>
      <td width='80' bgcolor='#ffffff' valign='bottom'>D</td></tr></table>"#);
    let t = table(&flow);
    assert_eq!(t.widths, vec![90.0, 144.0, 60.0]);
    assert_eq!(t.rows[0][0].text(), "A");
    assert_eq!(t.rows[0][0].rowspan, 2);
    assert_eq!(t.rows[0][1].colspan, 2);
    assert!(t.rows[1][0].covered);
    let cell = &t.rows[0][0];
    assert_eq!(cell.shading, Some(Rgb(240, 241, 242)));
    assert_eq!(cell.valign, VAlign::Center);
    let borders = cell.borders.unwrap();
    assert_eq!(borders.top.unwrap().style, BorderStyle::None);
    assert_eq!(borders.left.unwrap().style, BorderStyle::Dashed);
    assert_eq!(borders.left.unwrap().width, 2.0);
    assert_eq!(borders.left.unwrap().color, Some(Rgb(0x12, 0x34, 0x56)));
    assert!(!t.rows[0][1].borders.unwrap().any_visible());
    assert_eq!(t.rows[1][1].borders.unwrap().bottom.unwrap().color, Some(Rgb(1, 2, 3)));
    assert_eq!(t.rows[1][1].borders.unwrap().bottom.unwrap().width, 1.5);
    assert_eq!(t.rows[1][2].valign, VAlign::Bottom);
    let doc = model::to_doc(&flow);
    let Block::Table(t) = &*doc.body[0] else { panic!("expected table") };
    assert_eq!(t.grid, vec![90.0, 144.0, 60.0]);
    assert_eq!(t.rows[0].cells[0].props.valign, VAlign::Center);
    assert_eq!(t.rows[0].cells[0].props.borders, Some(borders));
}

#[test]
fn cell_width_hints_follow_rowspan_grid_positions() {
    let flow = parse("<table><tr><td rowspan=2 width=120>A<td>B<tr><td style='width:144pt'>C</table>");
    assert_eq!(table(&flow).widths, vec![90.0, 144.0]);
    let flow = parse("<table><tr><td colspan=2 style='width:200pt'>A<tr><td>B<td>C</table>");
    assert_eq!(table(&flow).widths, vec![100.0, 100.0]);
}

#[test]
fn google_docs_and_libreoffice_font_properties_reach_runs() {
    let flow = parse(r#"<p><span style="font-family:'Liberation Mono',monospace;
      font-size:16px;color:rgb(1, 2, 3);background:rgb(4, 5, 6);font-weight:600;
      font-style:italic;text-decoration:underline line-through">Styled</span>
      <span style='font-size:14pt;font-weight:500'>Regular</span></p>"#);
    let p = paragraph(&flow, 0);
    let f = run(p);
    assert_eq!(f.font.as_deref(), Some("Liberation Mono"));
    assert_eq!(f.size, Some(12.0));
    assert_eq!(f.color, Some(Rgb(1, 2, 3)));
    assert_eq!(f.background, Some(Rgb(4, 5, 6)));
    assert!(f.bold && f.italic && f.underline && f.strike);
    assert!(p.inlines.iter().any(|i| matches!(i, Inline::Text(t, f) if t == "Regular" && f.size == Some(14.0) && !f.bold)));
    let doc = model::to_doc(&flow);
    let Block::Para(p) = &*doc.body[0] else { panic!("expected paragraph") };
    assert_eq!(p.runs[0].props.font.as_deref(), Some("Liberation Mono"));
    assert_eq!(p.runs[0].props.shading, Some(Rgb(4, 5, 6)));
}

#[test]
fn hostile_html_limits_fall_back_to_inert_text() {
    let cases = [
        format!("{}Kept{}", "<div>".repeat(1000), "</div>".repeat(1000)),
        format!("<table><tr>{}</tr></table>", "<td>x</td>".repeat(100_000)),
        format!("<p style='{}'>Kept</p>", "x".repeat(css::MAX_STYLE + 1)),
        format!("<style>{}</style><p>Kept</p>", " ".repeat(css::MAX_CSS + 1)),
        format!("<style>{}</style><p>Kept</p>", "p{color:red}".repeat(2049)),
    ];
    for html in cases {
        assert!(parse_bounded(&html, &|_| None).is_err());
        let flow = parse(&html);
        assert_eq!(flow.blocks.len(), 1);
        let p = paragraph(&flow, 0);
        assert!(p.text().contains("Kept") || p.text().contains('x'));
        assert!(p.list.is_none());
        assert!(p.inlines.iter().all(|i| matches!(i, Inline::Text(_, f) if *f == Fmt::default())));
    }
    let html = format!("<p>{}</p>", "x".repeat(safety::MAX_INPUT));
    assert!(parse_bounded(&html, &|_| None).is_err());
    assert!(paragraph(&parse(&html), 0).text().len() <= safety::MAX_INPUT);
}

#[test]
fn rowspan_expansion_respects_total_cell_budget() {
    let html = format!("<table><tr>{}</tr>{}</table>", "<td rowspan=1000>x</td>".repeat(63), "<tr><td>y</td></tr>".repeat(999));
    assert!(parse_bounded(&html, &|_| None).is_err());
    assert!(matches!(parse(&html).blocks[0], FBlock::Para(_)));
}

#[test]
fn unsafe_content_and_remote_images_never_survive_import() {
    let calls = std::cell::Cell::new(0);
    let flow = parse_with(r#"<style>@import 'https://invalid.test/x';
      p { background:url('https://invalid.test/track'); behavior:expression(alert(1)); }
      @font-face { font-family:Remote; src:url('https://invalid.test/font'); }</style>
      <script>alert('hidden')</script><p onclick='alert(1)'>Safe
      <a href='javascript:alert(1)'>Script</a><a href='data:text/html,bad'>Data</a>
      <img src='https://invalid.test/img' onerror='alert(1)'>
      <img src='//invalid.test/img'><img src='file:///private/image.png'>
      <a href='https://example.test'>Link</a></p>"#, &|_| { calls.set(calls.get() + 1); None });
    assert_eq!(calls.get(), 0);
    let p = paragraph(&flow, 0);
    assert!(!p.text().contains("hidden"));
    assert!(p.inlines.iter().all(|i| matches!(i, Inline::Text(_, _))));
    assert!(p.inlines.iter().all(|i| matches!(i, Inline::Text(_, f) if f.background.is_none() &&
        f.link.as_deref().is_none_or(|l| l == "https://example.test"))));
    let out = export_fragment(&model::to_doc(&flow));
    for dangerous in ["onclick", "onerror", "javascript:", "data:text", "invalid.test", "<script", "expression("] {
        assert!(!out.contains(dangerous), "{dangerous}: {out}");
    }
}
