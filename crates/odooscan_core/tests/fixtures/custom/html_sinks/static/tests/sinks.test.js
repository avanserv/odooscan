import { markup } from "@odoo/owl";

export function sinks(el, html, x) {
    el.innerHTML = html;
    el.outerHTML += html;
    $(el).html(html);
    console.log(markup(x));
    debugger;
}
