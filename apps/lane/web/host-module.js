// SPDX-License-Identifier: Apache-2.0
export default (_modules, call) => ({
  workspace(value, callback) {
    call("workspace", { value }).then(callback);
  },
  copyText(text, callback) {
    call("copyText", { text }).then(callback);
  },
  openURL(url, callback) {
    call("openURL", { url }).then(callback);
  },
});
