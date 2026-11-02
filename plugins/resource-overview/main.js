import { View, div } from "gpui-kit";
import { context, labels } from "helixlane:resource";

// This is a real gpui-shell View. Rust owns the dock and credentials.
// Both the root and nested resource objects are immutable snapshots.
export default class ResourceOverview extends View {
  init() {
    this.labels = Object.freeze(labels());
  }
  render() {
    const current = Object.freeze(context());
    if (current.resource) Object.freeze(current.resource);
    const resource = current.resource;
    return div().flex().flex_col().p_5().gap_3()
      .child(this.labels.title)
      .child(resource ? resource.name : this.labels.empty)
      .child(resource ? `${resource.host}:${resource.port}` : "—")
      .child(`${this.labels.view}: ${current.view_id}`)
      .child(this.labels.permissions);
  }
}
