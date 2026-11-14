import { Component, markup } from "@odoo/owl";

export class NoteField extends Component {
    get reported() {
        const name = this.props.name;
        const x = this.props.value;
        return [
            markup(this.props.record.data.note),
            markup(`<b>${name}</b>`),
            owl.markup(x),
        ];
    }

    get silent() {
        const name = this.props.name;
        return [
            markup("<b>x</b>"),
            markup(_t("<b>Hi</b>")),
            markup`<b>${name}</b>`,
            markup(),
        ];
    }

    render(el, html, row, data) {
        el.innerHTML = html;
        el.outerHTML += row;
        this.$el.html(data);
    }

    renderSilent(el, html) {
        el.innerHTML = "";
        el.innerHTML = "<p>" + "</p>";
        el.innerHTML = `<p></p>`;
        el.textContent = html;
        const read = el.innerHTML;
        this.$el.html();
        this.$el.html("");
        el.innerHTML = `<input ${html ? "checked" : ""}/>`;
        Diff2Html.html(html, {});
        return [read, markup(`<b>${_t("Hi")}</b>`)];
    }
}
