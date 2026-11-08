export function Unconfigured() {
  return (
    <div class="empty" style={{ height: "100%" }}>
      <div class="illum" />
      <h2>No server to talk to</h2>
      <p>
        Start <code>atelierd</code> on this machine, then reload. (Pairing with a remote server is coming.)
      </p>
    </div>
  );
}
