// SPDX-License-Identifier: Apache-2.0
// Test-only desktop updater transport for the browser renderer. This fixture
// never downloads, installs, restarts or enters production build output.
export const bridgeModule = `export default (_modules, call) => ({
  call(channel, request, callback) {
    call("call", { channel, request }).then((value) => callback(JSON.stringify(value)));
  }
});`;
export function withUpdateFixture(host) {
  return host
    .replace(
      "view.nativeModulesMap = {",
      'view.nativeModulesMap = { bridge: new URL("./test-update-module.js", location.href).href,',
    )
    .replace(
      "view.onNativeModulesCall = async (name, data, moduleName) => {",
      `window.laneUpdateFixture = { phase: "idle", installedRelease: "delino@v1.2.3", message: "Update Lane to the latest version. The app will restart when the update is ready.", starts: 0, cancels: 0 };
view.onNativeModulesCall = async (name, data, moduleName) => {
  if (moduleName === "bridge") {
    const fixture = window.laneUpdateFixture;
    const action = data.request.args[0];
    if (action === "start") { fixture.starts++; fixture.phase = "checking"; fixture.message = "Checking for updates…"; }
    if (action === "cancel") { fixture.cancels++; fixture.phase = "idle"; }
    return { ok: true, response: { ...fixture } };
  }`,
    );
}
