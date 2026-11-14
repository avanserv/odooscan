/** @odoo-module **/
const { useState } = owl.hooks;

export function useCounter() {
    return useState({ value: 0 });
}
