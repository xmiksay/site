// Default page layout — brand-driven background/text/font (issue #68).
// Overrides mdcast's embedded default of the same name; every accessor below
// falls back to that prior hardcoded value, so a brand with no matching key
// renders identically to the un-overridden layout.
#import "/context.typ": doc-meta, brand-color, brand-font

#let layout(body) = [
  #set page(
    margin: 2cm,
    fill: brand-color("background", default: white),
    header: if doc-meta.title != "" or "classification" in doc-meta [
      #set text(size: 9pt, fill: brand-color("muted", default: luma(120)))
      #grid(
        columns: (1fr, 1fr),
        align(left)[#doc-meta.title],
        align(right)[#doc-meta.at("classification", default: "")],
      )
    ],
  )
  #set text(
    font: brand-font("sans", default: "New Computer Modern"),
    size: 11pt,
    fill: brand-color("text", default: black),
  )
  #eval(body, mode: "markup")
]
