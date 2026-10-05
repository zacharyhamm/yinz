import yinz, { type NotificationDecision } from "yinz:plugin";

// Bind with: hooks { notification "file:/path/to/notification-policy.ts"; }
export default {
  load(): void {
    yinz.subscribe(["NotificationRequested"]);
    yinz.on("NotificationRequested", (notification) => {
      let decision: NotificationDecision = "Pass";
      if (notification.body.startsWith("debug:")) {
        decision = "Suppress";
      } else if (notification.body.includes("build finished")) {
        decision = { Replace: { title: "Build", body: "Ready to review", delivery: "Both" } };
      } else if (notification.source !== "Toast") {
        decision = { Replace: { delivery: "Toast" } };
      }
      yinz.host.resolveNotification({ id: notification.id, decision });
    });
  },
};
