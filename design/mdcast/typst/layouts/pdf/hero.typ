// Hero / cover page — brand-driven background/heading/font (issue #68).
// Overrides mdcast's embedded default of the same name; every accessor below
// falls back to that prior hardcoded value, so a brand with no matching key
// renders identically to the un-overridden layout.
#import "/context.typ": doc-meta, brand-color, brand-font

#let layout(body) = [
  #set page(
    margin: (top: 6cm, bottom: 4cm, x: 3cm),
    fill: brand-color("background", default: white),
  )
  #set text(font: brand-font("sans", default: "New Computer Modern"), size: 14pt)
  #align(center)[
    #text(size: 28pt, weight: "bold", fill: brand-color("heading", default: black))[#eval(body, mode: "markup")]
    #if doc-meta.author != "" or doc-meta.date != "" [
      #v(0.5cm)
      #text(size: 12pt, fill: brand-color("text", default: black))[
        #doc-meta.author
        #if doc-meta.author != "" and doc-meta.date != "" [ · ]
        #doc-meta.date
      ]
    ]
  ]
]
