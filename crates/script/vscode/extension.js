// Connects VS Code's debugger to the script debugger inside the running game, which speaks the
// Debug Adapter Protocol on a local TCP port (crates/script/src/dap.rs).
const vscode = require('vscode');

function activate(context) {
  context.subscriptions.push(
    vscode.debug.registerDebugAdapterDescriptorFactory('engine-luau', {
      createDebugAdapterDescriptor(session) {
        const config = session.configuration;
        return new vscode.DebugAdapterServer(config.port || 4711, config.host || '127.0.0.1');
      },
    }),
  );
}

function deactivate() {}

module.exports = { activate, deactivate };
