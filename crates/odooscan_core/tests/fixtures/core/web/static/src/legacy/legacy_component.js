export class LegacyComponent extends owl.Component {
    trigger(name, payload) {
        this.env.bus.trigger(name, payload);
    }
}
