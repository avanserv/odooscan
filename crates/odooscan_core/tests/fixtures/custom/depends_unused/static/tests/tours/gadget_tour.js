import { startGadget } from "@prov_tour/../lib/gadget_lib";

export const gadgetTour = { steps: () => [{ trigger: "body", run: startGadget }] };
