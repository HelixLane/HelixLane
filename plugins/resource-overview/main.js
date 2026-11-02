import { View, div } from "gpui-kit";
import { context, labels } from "helixlane:resource";

// This is a real gpui-shell View. Rust owns the dock and credentials.
// Both the root and nested resource objects are immutable snapshots.
export default class ResourceOverview extends View {
  init() {
    this.context = Object.freeze(context());
    if (this.context.resource) Object.freeze(this.context.resource);
    this.labels = Object.freeze(labels());
  }
  render() {
    const resource = this.context.resource;
    return div().flex().flex_col().p_5().gap_3()
      .child(this.labels.title)
      .child(resource ? resource.name : this.labels.empty)
      .child(resource ? `${resource.host}:${resource.port}` : "—")
      .child(`${this.labels.view}: ${this.context.view_id}`)
      .child(this.labels.permissions);
  }
}
