// Closing page — brand-driven background/heading/font (issue #68). Overrides
// mdcast's embedded default of the same name; every accessor below falls
// back to that prior hardcoded value, so a brand with no matching key
// renders identically to the un-overridden layout.
#import "/context.typ": brand-color, brand-font

#let layout(body) = [
  #set page(
    margin: (top: 8cm, bottom: 4cm, x: 3cm),
    fill: brand-color("background", default: white),
  )
  #set text(font: brand-font("sans", default: "New Computer Modern"))
  #align(center)[
    #text(size: 24pt, weight: "bold", fill: brand-color("heading", default: black))[Thank you]
    #v(1cm)
    #text(size: 14pt, fill: brand-color("text", default: black))[#eval(body, mode: "markup")]
  ]
]
