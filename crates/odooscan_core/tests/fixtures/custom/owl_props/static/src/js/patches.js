import { patch } from "@web/core/utils/patch";
import { ListCard, Patchy } from "@owl_props/js/card";
import { Shelf } from "@owl_props/js/board";

Patchy.props.extra = { type: String, optional: true };

patch(Shelf, {
    components: { ...Shelf.components, ListCard },
});
