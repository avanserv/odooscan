import { patch } from "@web/core/utils/patch";
import { BadgeCopy, ChipCopy, ListCard, Patchy, TileAlias } from "@owl_props/js/card";
import { Shelf } from "@owl_props/js/board";

Patchy.props.extra = { type: String, optional: true };

patch(Shelf, {
    components: { ...Shelf.components, ListCard },
});

TileAlias.props.extra = { type: String, optional: true };
BadgeCopy.props.title.optional = true;
ChipCopy.props.extra = { type: String, optional: true };
