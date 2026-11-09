/** Cancellation stops accepting rows; the accepted queue drains before settlement. */
export class ExportJob {
  state: "running" | "draining" | "cancelled" = "running";
  pendingRows = 2;
  cancel() {
    this.state = "draining";
  }
  tick() {
    if (this.state === "draining" && --this.pendingRows === 0) {
      this.state = "cancelled";
    }
  }
}

/** CLI interaction surface. */
export function cancelExport(job: ExportJob) {
  job.cancel();
  return "Export cancelled";
}

export const help = "export start: begin export; export cancel: cancel export";
