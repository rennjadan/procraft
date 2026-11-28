# Material Symbols' undo and redo

`undo.svg` and `redo.svg` are Google's Material Symbols "undo" and "redo"
(<https://fonts.google.com/icons>, <https://github.com/google/material-design-icons>), in the
Outlined style with fill 0, weight 400, grade 0 and optical size 24: the 24 px SVGs as Google
serves them from fonts.gstatic.com, kept as given (their 960-unit grid, `viewBox="0 -960 960 960"`).
Josh, ProCraft's maintainer, chose them for the side bar's Undo and Redo.

`tools/icons/draw_icons.py` puts each on the 24-unit grid and places it in its slot like the other
icons (nothing widened or rounded), as `assets/icons/procraft/undo.svg`, `redo.svg` and `pieundo.svg`
(the adjustment menus' Undo, the same arrow) and in the apps' icon tables: `PcIcons.Undo`, `Redo` and
`PieUndo` in the Android app's `Icons.kt`, and from there `UNDO`, `REDO` and `PIE_UNDO` in
`desktop/src/icons.rs` and `undo`, `redo` and `pieUndo` in `ios/ProCraft/Icons.swift`.

Licence: Apache License 2.0, as Google gives the set; its text is `LICENSE` beside them, as in
Google's repository.
