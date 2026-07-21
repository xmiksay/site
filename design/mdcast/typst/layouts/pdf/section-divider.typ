// Section divider — brand-driven background/heading/font (issue #68).
// Overrides mdcast's embedded default of the same name; every accessor below
// falls back to that prior hardcoded value, so a brand with no matching key
// renders identically to the un-overridden layout.
#import "/context.typ": brand-color, brand-font

#let layout(body) = [
  #set page(margin: 4cm, fill: brand-color("background", default: rgb("#f5f5f5")))
  #set text(font: brand-font("sans", default: "New Computer Modern"))
  #align(center + horizon)[
    #text(size: 32pt, weight: "bold", fill: brand-color("heading", default: black))[#eval(body, mode: "markup")]
  ]
]
