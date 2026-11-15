/** @odoo-module **/
const { useState } = owl;

owl.hooks = { useState };

export const pickState = owl.hooks ? owl.hooks.useState : owl.useState;

const { useRef } = owl.hooks || {};

export { useRef };
