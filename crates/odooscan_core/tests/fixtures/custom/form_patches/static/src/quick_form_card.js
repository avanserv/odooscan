import { Component } from "@odoo/owl";

export class QuickFormCard extends Component {
    static template = "form_patches.QuickFormCard";
}

export class QuickFormBadge extends Component {
    static template = "form_patches.QuickFormBadge";
}

export class QuickFormBase extends Component {
    static template = "form_patches.Unrendered";
}

export class QuickFormLine extends QuickFormBase {
    static template = "form_patches.QuickFormCard";
}
