// A normal file plugin supplies the authenticated caller identity for session.rpc.
import z, {definePlugin} from 'yinz:plugin';
let ready: Promise<unknown>;
export default definePlugin({
  load() { ready = z.waitForPermissions(['ReadCliPipes', 'MessageAndLaunchOtherPlugins']); },
  async pipe(message) {
    if (message.name !== 'test-rpc' || !message.payload || typeof message.source !== 'object' || !('Cli' in message.source)) return;
    const pipe = message.source.Cli;
    z.command('BlockCliPipeInput', pipe);
    let response: string;
    try {
      await ready;
      const result = await z.rpc.call(JSON.parse(message.payload));
      response = JSON.stringify({Ok: result});
      if (z.encodeUtf8(response).length > 32768) throw Error('test RPC reply exceeds 32 KiB');
    } catch (error) { response = JSON.stringify({Err: String(error).slice(0, 2000)}); }
    z.command('CliPipeOutput', [pipe, response]);
    z.command('UnblockCliPipeInput', pipe);
  },
});
