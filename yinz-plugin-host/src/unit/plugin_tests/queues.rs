//! Work queues, end to end: an enqueuer plugin, the real plugin thread, headless job plugins,
//! and the events that come back.
//!
//! Every test here drives a TypeScript enqueuer written to a tempdir, because that is the cheapest way
//! to get a plugin that can be told what to do and can say what it saw: it logs one line per
//! event and renders the log, so a test waits for the *content* it means rather than for a render
//! count. The fake screen is a plain loop rather than the shared macros because it has to grant
//! permissions to whichever plugin asks - the jobs and a second subscriber have ids of their own.

use super::*;
use crate::ClientId;
use crate::PaneId;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;
use yinz_types::keys::{BareKey, KeyWithModifier};
use {yinz_types::data::MessageToPlugin, yinz_types::data::PermissionStatus};

const WAIT: std::time::Duration = std::time::Duration::from_secs(20);

#[test]
fn deno_jobs_survive_enqueuer_reload_but_addressed_events_keep_the_original_generation() {
    let harness = Harness::new(PermissionStatus::Granted);
    let job = harness.write_plugin("job.ts", r#"
export default {
  load(config) { if (config.n === '2') yinz.command('FinishQueueJob', {success: true, payload: 'second'}); },
  pipe(message) { if (message.name === 'finish') yinz.command('FinishQueueJob', {success: true, payload: 'first'}); }
};
"#);
    let enqueuer = harness.write_plugin("enqueuer.ts", &format!(r#"
let lines = [];
export default {{
  async load() {{
    yinz.subscribe(['QueueJobStarted', 'QueueJobFinished', 'QueueChanged']);
    await yinz.waitForPermissions(['MessageAndLaunchOtherPlugins', 'ReadApplicationState']);
    for (const n of ['1', '2']) yinz.command('EnqueueQueueJob', {{queue: 'deno', plugin_url: {job:?}, config: {{n}}, context: {{n}}}});
  }},
  update(event) {{
    if (event.QueueJobStarted) {{ const e = event.QueueJobStarted; lines.push(`started ${{e.job_id}} pid=${{e.plugin_id}} n=${{e.context.n}}`); }}
    return true;
  }},
  render() {{ return lines.join('\n'); }}
}};
"#));
    harness.load(&enqueuer);
    let first = harness.wait_for_started(1);
    harness.write_plugin(
        "enqueuer.ts",
        r#"
let state = 'replacement loading', addressed = 0;
export default {
  async load() {
    yinz.subscribe(['QueueJobStarted', 'QueueJobFinished', 'QueueChanged']);
    await yinz.waitForPermissions(['ReadApplicationState']);
    const q = yinz.command('QueueStatus', {queue: 'deno'})[0];
    state = `replacement p=${q.pending.length} r=${q.running.length}`;
  },
  update(event) {
    if (event.QueueJobStarted || event.QueueJobFinished) addressed++;
    if (event.QueueChanged) {
      const q = yinz.command('QueueStatus', {queue: 'deno'})[0];
      state = `replacement p=${q.pending.length} r=${q.running.length} addressed=${addressed}`;
    }
    return true;
  },
  render() { return state; }
};
"#,
    );
    let run =
        yinz_runtime::plugin_location::run_plugin_or_alias_from_url(&enqueuer, &None, None, None)
            .unwrap();
    harness
        .to_plugin
        .send(PluginInstruction::ReloadPluginByUrl(run))
        .unwrap();
    harness.wait_for("replacement p=1 r=1");
    harness.pipe_to(first, "finish");
    harness.wait_for("replacement p=0 r=0 addressed=0");
}

/// What the enqueuer does on each key; see `enqueuer_source`.
const ENQUEUE: char = 'e';
const ENQUEUE_BAD_URL: char = 'b';
const RAISE_CONCURRENCY: char = 'c';
const CANCEL_JOB_1: char = '1';
const CANCEL_JOB_2: char = '2';
const ENQUEUE_AND_CANCEL: char = 'x';
const STATUS: char = 's';

fn enqueuer_source(job_url: &str) -> String {
    format!(
        r#"
import yinz from 'yinz:plugin';
const lines=[];
let n=0, last;
const log = s => lines.push(s);
function enqueue(url) {{
  n++;
  try {{ last=yinz.command('EnqueueQueueJob', {{queue:'merge',plugin_url:url,config:{{n:String(n)}},context:{{n:String(n)}}}}); log('enqueued '+last); }}
  catch (error) {{log('enqueue error: '+error);}}
}}
yinz.on('QueueJobStarted', e => {{log(`started ${{e.job_id}} pid=${{e.plugin_id}} n=${{e.context.n}}`); return true;}});
yinz.on('QueueJobFinished', e => {{log(`finished ${{e.job_id}} ${{e.outcome}} payload=${{e.payload ?? 'nil'}} n=${{e.context.n}}`); return true;}});
yinz.on('QueueChanged', e => {{log(`changed ${{e.queue}} p=${{e.pending.length}} r=${{e.running.length}} c=${{e.concurrency}}`); return true;}});
export default {{
  load() {{yinz.subscribe(['Key','QueueJobStarted','QueueJobFinished','QueueChanged']); yinz.requestPermission(['MessageAndLaunchOtherPlugins','ChangeApplicationState','ReadApplicationState']); log('ready');}},
  update(event) {{
    if (!event.Key) return false;
    const k=yinz.event(event).key;
    if (k === '{ENQUEUE}') enqueue('{job_url}');
    else if (k === '{ENQUEUE_BAD_URL}') enqueue('file:/nonexistent/job.ts');
    else if (k === '{RAISE_CONCURRENCY}') yinz.command('ConfigureQueue', {{queue:'merge',concurrency:2}});
    else if (k === '{CANCEL_JOB_1}') yinz.command('CancelQueueJob', {{job_id:1}});
    else if (k === '{CANCEL_JOB_2}') yinz.command('CancelQueueJob', {{job_id:2}});
    else if (k === '{ENQUEUE_AND_CANCEL}') {{enqueue('{job_url}'); yinz.command('CancelQueueJob', {{job_id:last}});}}
    else if (k === '{STATUS}') {{
      const q=yinz.command('QueueStatus', {{queue:'merge'}})[0];
      log(`status p=${{q.pending.length}} r=${{q.running.length}} c=${{q.concurrency}} first_pending=${{q.pending[0]?.job_id ?? 'nil'}} running_url=${{q.running[0]?.plugin_url ?? 'nil'}}`);
    }}
    return true;
  }},
  pipe(message) {{log('pipe '+message.name); return true;}},
  render() {{return lines.join('\n');}}
}};
        "#
    )
}

/// A job that tells its enqueuer when it has loaded, then waits to be finished or pinged. It
/// cannot render - a job has no pane - so a message is the only way to see it run.
const CHATTY_JOB: &str = r#"
import yinz from 'yinz:plugin';
let enqueuer;
function tell(name) {yinz.command('MessageToPlugin', {destination_plugin_id:enqueuer,message_name:name});}
export default {
  async load(config) {enqueuer=Number(config.yinz_queue_enqueuer_plugin_id); await yinz.waitForPermissions(['MessageAndLaunchOtherPlugins']); tell('job_loaded');},
  pipe(message) {if (message.name === 'ping') tell('job_alive'); else if (message.name === 'finish') yinz.command('FinishQueueJob',{success:true}); return false;}
};
"#;

/// A job that reports as soon as it is loaded.
const FINISHING_JOB: &str = r#"
import yinz from 'yinz:plugin';
export default {load(config) {if (!yinz.getPluginInfo().background) throw new Error('queue job must be background during load'); yinz.command('FinishQueueJob', {success:true,payload:`hello from ${config.yinz_queue}/${config.yinz_queue_job_id} n=${config.n} by=${config.yinz_queue_enqueuer_plugin_id}`});}};
"#;

/// A job that runs until told to stop, one way or another.
const WAITING_JOB: &str = r#"
import yinz from 'yinz:plugin';
export default {pipe(message) {if (message.name === 'finish') yinz.command('FinishQueueJob',{success:false,payload:'gave up'}); else if (message.name === 'close') yinz.host.closeSelf(); return false;}};
"#;

const CRASHING_JOB: &str = r#"
export default {load() {throw Error('boom');}};
"#;

/// A bystander that only listens for queue changes.
const SUBSCRIBER: &str = r#"
import yinz from 'yinz:plugin';
const lines=[];
yinz.on('QueueChanged', e => {lines.push(`bystander saw ${e.queue} p=${e.pending.length} r=${e.running.length}`); return true;});
export default {load() {yinz.subscribe(['QueueChanged']); yinz.requestPermission(['ReadApplicationState']);}, render() {return lines.join('\n');}};
"#;

struct Harness {
    to_plugin: SenderWithContext<PluginInstruction>,
    log: Watched<Vec<ScreenInstruction>>,
    screen_thread: Option<std::thread::JoinHandle<()>>,
    teardown: Option<Box<dyn FnOnce()>>,
    client_id: ClientId,
    dir: tempfile::TempDir,
}

impl Harness {
    /// A plugin thread with a fake screen that answers every permission request with `answer`.
    fn new(answer: PermissionStatus) -> Self {
        let dir = tempdir().unwrap();
        let plugin_host_folder = PathBuf::from(dir.path());
        let cache_path = plugin_host_folder.join("permissions_test.kdl");
        let (to_plugin, screen_receiver, teardown) =
            create_plugin_thread(Some(plugin_host_folder), None);
        let client_id = ClientId::new(1);
        let log = Watched::new(vec![]);
        let screen_thread = std::thread::Builder::new()
            .name("fake_screen_thread".to_string())
            .spawn({
                let log = log.clone();
                let to_plugin = to_plugin.clone();
                move || loop {
                    let Ok((instruction, _)) = screen_receiver.recv_timeout(WAIT * 3) else {
                        break;
                    };
                    match instruction {
                        ScreenInstruction::Exit => break,
                        ScreenInstruction::RequestPluginPermissions(_plugin_id, permission) => {
                            let _ = to_plugin.send(PluginInstruction::PermissionAnswer {
                                request_id: permission.request_id,
                                status: answer,
                                cache_path: Some(cache_path.clone()),
                            });
                        },
                        other => log.lock().push(other),
                    }
                }
            })
            .unwrap();
        let _ = to_plugin.send(PluginInstruction::AddClient(client_id));
        Harness {
            to_plugin,
            log,
            screen_thread: Some(screen_thread),
            teardown: Some(teardown),
            client_id,
            dir,
        }
    }

    fn write_plugin(&self, name: &str, source: &str) -> String {
        let path = self.dir.path().join(name);
        std::fs::write(&path, source).unwrap();
        format!("file:{}", path.display())
    }

    /// A second (or third) client attaching to the session.
    fn add_client(&self, client_id: ClientId) {
        let _ = self.to_plugin.send(PluginInstruction::AddClient(client_id));
    }

    /// Load a plugin into a pane, as the enqueuer is.
    fn load(&self, url: &str) {
        self.load_with_configuration(url, &[]);
    }

    fn load_with_configuration(&self, url: &str, configuration: &[(&str, &str)]) {
        let mut run_plugin =
            yinz_runtime::plugin_location::run_plugin_or_alias_from_url(url, &None, None, None)
                .unwrap();
        if let RunPluginOrAlias::RunPlugin(run_plugin) = &mut run_plugin {
            for (key, value) in configuration {
                run_plugin.configuration.insert(*key, *value);
            }
        }
        let _ = self.to_plugin.send(PluginInstruction::Load(
            Some(false),
            false,
            false,
            Some("queue_test_plugin".to_owned()),
            run_plugin,
            Some(TabId::new(1)),
            None,
            self.client_id,
            Size { cols: 80, rows: 20 },
            None,
            None,
            false,
            None,
            None,
            None,
            None,
        ));
    }

    fn key(&self, c: char) {
        let _ = self.to_plugin.send(PluginInstruction::Update(vec![(
            None,
            Some(self.client_id),
            Event::Key(KeyWithModifier::new(BareKey::Char(c))),
        )]));
    }

    /// Pipe a message straight to one job plugin, the way another plugin would.
    fn pipe_to(&self, plugin_id: yinz_types::internal_ids::PluginId, name: &str) {
        let _ = self.to_plugin.send(PluginInstruction::MessageFromPlugin {
            source_plugin_id: crate::PluginId::new(0),
            message: MessageToPlugin {
                destination_plugin_id: Some(plugin_id),
                ..MessageToPlugin::new(name)
            },
        });
    }

    /// Every frame any plugin rendered so far.
    fn frames(&self) -> Vec<String> {
        rendered_frames(&self.log.lock())
    }

    /// Block until some rendered frame contains `needle`, and return that frame.
    fn wait_for(&self, needle: &str) -> String {
        self.log.wait_until_described(
            &format!("a render containing {needle:?}"),
            WAIT,
            |instructions| {
                rendered_frames(instructions)
                    .into_iter()
                    .find(|frame| frame.contains(needle))
            },
            |instructions| format!("{:#?}", rendered_frames(instructions)),
        )
    }

    /// The job's plugin id, from the enqueuer's `started` line.
    fn wait_for_started(&self, job_id: u64) -> crate::PluginId {
        let frame = self.wait_for(&format!("started {job_id} pid="));
        started_plugin_id(&frame, job_id).unwrap()
    }

    fn close_pane_instructions(&self) -> Vec<PaneId> {
        self.log
            .lock()
            .iter()
            .filter_map(|instruction| match instruction {
                ScreenInstruction::ClosePane(pane_id, ..) => Some(*pane_id),
                _ => None,
            })
            .collect()
    }

    fn suppressed_add_plugin_instructions(&self) -> Vec<yinz_types::internal_ids::PluginId> {
        self.log
            .lock()
            .iter()
            .filter_map(|instruction| match instruction {
                ScreenInstruction::AddPlugin(_, _, _, _, _, _, plugin_id, _, _, true, ..) => {
                    Some(*plugin_id)
                },
                _ => None,
            })
            .collect()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(teardown) = self.teardown.take() {
            teardown();
        }
        if let Some(screen_thread) = self.screen_thread.take() {
            let _ = screen_thread.join();
        }
    }
}

/// The plugin id in the enqueuer's `started <job_id> pid=<plugin_id> ...` line, if `frame` has it.
fn started_plugin_id(frame: &str, job_id: u64) -> Option<crate::PluginId> {
    let prefix = format!("started {job_id} pid=");
    let line = frame.lines().find(|line| line.starts_with(&prefix))?;
    line[prefix.len()..]
        .split_whitespace()
        .next()
        .and_then(|plugin_id| plugin_id.parse().ok())
}

fn rendered_frames(instructions: &[ScreenInstruction]) -> Vec<String> {
    instructions
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::PluginBytes(assets) => Some(assets),
            _ => None,
        })
        .flatten()
        .map(|asset| String::from_utf8_lossy(&asset.bytes).replace("\n\r", "\n"))
        .filter(|rendered| !rendered.is_empty())
        .collect()
}

fn assert_never_rendered(harness: &Harness, needle: &str) {
    let frames = harness.frames();
    assert!(
        !frames.iter().any(|frame| frame.contains(needle)),
        "expected no frame to contain {needle:?}, got: {frames:#?}"
    );
}

#[test]
#[serial(plugin_tests)]
pub fn a_job_runs_headless_and_reports_back() {
    let harness = Harness::new(PermissionStatus::Granted);
    let job = harness.write_plugin("job.ts", FINISHING_JOB);
    let enqueuer = harness.write_plugin("enqueuer.ts", &enqueuer_source(&job));
    harness.load(&enqueuer);
    harness.key(ENQUEUE);

    harness.wait_for("enqueued 1");
    let job_plugin_id = harness.wait_for_started(1);
    let frame = harness.wait_for("finished 1 Succeeded");
    assert!(
        frame.contains("finished 1 Succeeded payload=hello from merge/1 n=1 by=0 n=1"),
        "the job's payload and the enqueuer's context both come back: {frame}"
    );
    harness.wait_for("changed merge p=0 r=0 c=1");

    assert_eq!(
        harness.suppressed_add_plugin_instructions(),
        vec![job_plugin_id],
        "the job got a suppressed pane and nothing else"
    );
    harness.log.wait_until("the job's pane to be closed", WAIT, |instructions| {
        instructions
            .iter()
            .any(|i| matches!(i, ScreenInstruction::ClosePane(PaneId::Plugin(id), ..) if *id == job_plugin_id))
            .then_some(())
    });
}

#[test]
#[serial(plugin_tests)]
pub fn concurrency_one_serialises_jobs() {
    let harness = Harness::new(PermissionStatus::Granted);
    let job = harness.write_plugin("job.ts", WAITING_JOB);
    let enqueuer = harness.write_plugin("enqueuer.ts", &enqueuer_source(&job));
    harness.load(&enqueuer);
    harness.key(ENQUEUE);
    harness.key(ENQUEUE);

    let first = harness.wait_for_started(1);
    harness.wait_for("changed merge p=1 r=1 c=1");
    assert_never_rendered(&harness, "started 2");

    harness.pipe_to(first, "finish");
    let frame = harness.wait_for("finished 1 Failed");
    assert!(
        frame.contains("finished 1 Failed payload=gave up n=1"),
        "{frame}"
    );
    let second = harness.wait_for_started(2);
    assert_ne!(first, second);
    harness.pipe_to(second, "finish");
    harness.wait_for("finished 2 Failed");
    harness.wait_for("changed merge p=0 r=0 c=1");
}

#[test]
#[serial(plugin_tests)]
pub fn raising_concurrency_releases_pending_jobs() {
    let harness = Harness::new(PermissionStatus::Granted);
    let job = harness.write_plugin("job.ts", WAITING_JOB);
    let enqueuer = harness.write_plugin("enqueuer.ts", &enqueuer_source(&job));
    harness.load(&enqueuer);
    harness.key(ENQUEUE);
    harness.key(ENQUEUE);

    let first = harness.wait_for_started(1);
    harness.wait_for("changed merge p=1 r=1 c=1");
    harness.key(RAISE_CONCURRENCY);
    let second = harness.wait_for_started(2);
    harness.wait_for("changed merge p=0 r=2 c=2");
    assert_never_rendered(&harness, "finished 1");

    harness.pipe_to(first, "finish");
    harness.pipe_to(second, "finish");
    harness.wait_for("finished 2 Failed");
}

#[test]
#[serial(plugin_tests)]
pub fn cancelling_a_pending_job_reports_cancelled_and_never_starts_it() {
    let harness = Harness::new(PermissionStatus::Granted);
    let job = harness.write_plugin("job.ts", WAITING_JOB);
    let enqueuer = harness.write_plugin("enqueuer.ts", &enqueuer_source(&job));
    harness.load(&enqueuer);
    harness.key(ENQUEUE);
    harness.key(ENQUEUE);

    let first = harness.wait_for_started(1);
    harness.wait_for("enqueued 2");
    harness.key(CANCEL_JOB_2);
    let frame = harness.wait_for("finished 2 Cancelled");
    assert!(
        frame.contains("finished 2 Cancelled payload=nil n=2"),
        "{frame}"
    );
    harness.wait_for("changed merge p=0 r=1 c=1");

    harness.pipe_to(first, "finish");
    harness.wait_for("finished 1 Failed");
    harness.wait_for("changed merge p=0 r=0 c=1");
    assert_never_rendered(&harness, "started 2");
}

#[test]
#[serial(plugin_tests)]
pub fn cancelling_a_running_job_unloads_it_and_starts_the_next() {
    let harness = Harness::new(PermissionStatus::Granted);
    let job = harness.write_plugin("job.ts", WAITING_JOB);
    let enqueuer = harness.write_plugin("enqueuer.ts", &enqueuer_source(&job));
    harness.load(&enqueuer);
    harness.key(ENQUEUE);
    harness.key(ENQUEUE);

    let first = harness.wait_for_started(1);
    harness.key(CANCEL_JOB_1);
    harness.wait_for("finished 1 Cancelled");
    let second = harness.wait_for_started(2);
    assert!(harness
        .close_pane_instructions()
        .contains(&PaneId::Plugin(first)));

    harness.pipe_to(second, "finish");
    harness.wait_for("finished 2 Failed");
}

#[test]
#[serial(plugin_tests)]
pub fn a_job_is_one_instance_no_matter_how_many_clients_are_attached() {
    // a job is loaded `shared`: the client attached when it starts does not get it cloned, and
    // neither does one attaching while it runs. otherwise every client's copy would run the job.
    // the enqueuer is shared too, so that its log comes from one instance.
    let harness = Harness::new(PermissionStatus::Granted);
    let job = harness.write_plugin("job.ts", CHATTY_JOB);
    let enqueuer = harness.write_plugin("enqueuer.ts", &enqueuer_source(&job));
    harness.load_with_configuration(&enqueuer, &[("shared", "true")]);
    harness.wait_for("ready");
    harness.add_client(crate::ClientId::new(2));
    harness.key(ENQUEUE);

    let job_plugin_id = harness.wait_for_started(1);
    harness.wait_for("pipe job_loaded");
    harness.add_client(crate::ClientId::new(3));
    harness.pipe_to(job_plugin_id, "finish");
    let frame = harness.wait_for("finished 1 Succeeded");
    assert_eq!(
        frame.matches("pipe job_loaded").count(),
        1,
        "the job loaded exactly once for two clients and a third that joined it: {frame}"
    );
}

#[test]
#[serial(plugin_tests)]
pub fn cancelling_a_job_before_its_loader_ran_leaves_no_orphan() {
    // a cancel that lands after the job was popped but before its loader took the plugin map
    // lock must stop the load, not just forget the job. the enqueuer cancels in the same update
    // that enqueues, so the cancel instruction sits on the bus directly behind the pump that
    // launches the job, and the main thread's unload races the executor thread's load. which
    // side wins is up to the scheduler, so this loops a few times, and both interleavings must
    // come out the same: the job is gone by the time its cancellation is reported, and a ping to
    // its plugin id reaches nothing.
    let harness = Harness::new(PermissionStatus::Granted);
    let job = harness.write_plugin("job.ts", CHATTY_JOB);
    let enqueuer = harness.write_plugin("enqueuer.ts", &enqueuer_source(&job));
    harness.load(&enqueuer);
    harness.wait_for("ready");

    let mut job_id = 0;
    for _ in 0..20 {
        job_id += 1;
        harness.key(ENQUEUE_AND_CANCEL);
        let frame = harness.wait_for(&format!("finished {job_id} Cancelled"));
        if let Some(plugin_id) = started_plugin_id(&frame, job_id) {
            harness.pipe_to(plugin_id, "ping");
        }
    }
    // an ordering sentinel: a job finished after every ping above was delivered
    job_id += 1;
    harness.key(ENQUEUE);
    let sentinel = harness.wait_for_started(job_id);
    harness.pipe_to(sentinel, "finish");
    harness.wait_for(&format!("finished {job_id} Succeeded"));
    assert_never_rendered(&harness, "pipe job_alive");
}

#[test]
#[serial(plugin_tests)]
pub fn a_job_that_closes_itself_is_aborted_and_frees_its_slot() {
    let harness = Harness::new(PermissionStatus::Granted);
    let job = harness.write_plugin("job.ts", WAITING_JOB);
    let enqueuer = harness.write_plugin("enqueuer.ts", &enqueuer_source(&job));
    harness.load(&enqueuer);
    harness.key(ENQUEUE);
    harness.key(ENQUEUE);

    let first = harness.wait_for_started(1);
    harness.pipe_to(first, "close");
    let frame = harness.wait_for("finished 1 Aborted");
    assert!(
        frame.contains("finished 1 Aborted payload=nil n=1"),
        "{frame}"
    );
    let second = harness.wait_for_started(2);
    harness.pipe_to(second, "finish");
    harness.wait_for("finished 2 Failed");
}

#[test]
#[serial(plugin_tests)]
pub fn a_job_that_raises_is_reported_failed_and_frees_its_slot() {
    let harness = Harness::new(PermissionStatus::Granted);
    let job = harness.write_plugin("job.ts", CRASHING_JOB);
    let enqueuer = harness.write_plugin("enqueuer.ts", &enqueuer_source(&job));
    harness.load(&enqueuer);
    harness.key(ENQUEUE);
    harness.key(ENQUEUE);

    harness.wait_for_started(1);
    let frame = harness.wait_for("finished 1 Failed");
    assert!(
        frame.contains("boom"),
        "the error is the payload of a job that raised: {frame}"
    );
    harness.wait_for_started(2);
    harness.wait_for("finished 2 Failed");
    harness.wait_for("changed merge p=0 r=0 c=1");
}

#[test]
#[serial(plugin_tests)]
pub fn a_job_that_fails_to_load_is_reported_failed_and_the_next_one_runs() {
    let harness = Harness::new(PermissionStatus::Granted);
    let job = harness.write_plugin("job.ts", FINISHING_JOB);
    let enqueuer = harness.write_plugin("enqueuer.ts", &enqueuer_source(&job));
    harness.load(&enqueuer);
    harness.key(ENQUEUE_BAD_URL);
    harness.key(ENQUEUE);

    harness.wait_for("finished 1 Failed");
    harness.wait_for_started(2);
    harness.wait_for("finished 2 Succeeded");
    harness.wait_for("changed merge p=0 r=0 c=1");
}

#[test]
#[serial(plugin_tests)]
pub fn queue_changed_is_broadcast_to_other_subscribers() {
    let harness = Harness::new(PermissionStatus::Granted);
    let job = harness.write_plugin("job.ts", FINISHING_JOB);
    let enqueuer = harness.write_plugin("enqueuer.ts", &enqueuer_source(&job));
    let subscriber = harness.write_plugin("subscriber.ts", SUBSCRIBER);
    harness.load(&enqueuer);
    harness.load(&subscriber);
    harness.wait_for("ready");
    harness.key(ENQUEUE);

    harness.wait_for("bystander saw merge p=0 r=1");
    harness.wait_for("bystander saw merge p=0 r=0");
}

#[test]
#[serial(plugin_tests)]
pub fn queue_status_answers_with_pending_and_running_jobs() {
    let harness = Harness::new(PermissionStatus::Granted);
    let job = harness.write_plugin("job.ts", WAITING_JOB);
    let enqueuer = harness.write_plugin("enqueuer.ts", &enqueuer_source(&job));
    harness.load(&enqueuer);
    harness.key(ENQUEUE);
    harness.key(ENQUEUE);

    let first = harness.wait_for_started(1);
    harness.wait_for("enqueued 2");
    harness.key(STATUS);
    let frame = harness.wait_for("status ");
    assert!(
        frame.contains(&format!(
            "status p=1 r=1 c=1 first_pending=2 running_url={job}"
        )),
        "{frame}"
    );

    harness.pipe_to(first, "finish");
    harness.wait_for("finished 1 Failed");
}

#[test]
#[serial(plugin_tests)]
pub fn enqueue_is_denied_without_permission() {
    let harness = Harness::new(PermissionStatus::Denied);
    let job = harness.write_plugin("job.ts", FINISHING_JOB);
    let enqueuer = harness.write_plugin("enqueuer.ts", &enqueuer_source(&job));
    harness.load(&enqueuer);
    harness.key(ENQUEUE);

    let frame = harness.wait_for("enqueue error");
    assert!(
        frame.contains("MessageAndLaunchOtherPlugins"),
        "the error names the missing permission: {frame}"
    );
    assert!(harness.suppressed_add_plugin_instructions().is_empty());
    assert_never_rendered(&harness, "started 1");
}

#[test]
#[serial(plugin_tests)]
pub fn a_wasm_plugin_can_enqueue_and_be_a_job() {
    let harness = Harness::new(PermissionStatus::Granted);
    let fixture = format!("file:{}", *PLUGIN_FIXTURE);
    harness.load(&fixture);
    // Broadcast pipes enumerate loaded plugins. The fixture's first frame proves it is
    // loaded; without this wait a slow WASM load can miss the enqueue request entirely.
    harness.wait_for("Received events:");
    // The fixture renders every event it receives, so a render mentioning a queue event is the
    // assertion. Ask it to queue *itself*.
    let _ = harness.to_plugin.send(PluginInstruction::CliPipe {
        focus_plugin: false,
        is_eof: false,
        pipe_id: "input_pipe_id".to_owned(),
        name: "enqueue_queue_job".to_owned(),
        payload: Some(fixture.clone()),
        plugin: None,
        args: None,
        configuration: None,
        floating: None,
        pane_id_to_replace: None,
        pane_title: None,
        cwd: None,
        skip_cache: false,
        cli_client_id: harness.client_id,
        source_pane_id: None,
    });

    harness.wait_for("enqueued job 1");
    let frame = harness.wait_for("QueueJobStarted");
    assert!(frame.contains("\"origin\": \"fixture\""), "{frame}");
    let job_plugin_id = *harness
        .suppressed_add_plugin_instructions()
        .last()
        .expect("the job was launched headless");
    harness.pipe_to(job_plugin_id, "finish_queue_job");
    let frame = harness.wait_for("QueueJobFinished");
    assert!(frame.contains("Succeeded"), "{frame}");
}

#[test]
fn first_cli_pipe_and_reload_reach_deno_without_an_attached_ui() {
    let harness = Harness::new(PermissionStatus::Granted);
    let plugin = harness.write_plugin(
        "detached.ts",
        r#"
let message = 'not delivered';
export default {pipe(event) { message = 'received ' + event.payload; }, render() {return message;}};
"#,
    );
    harness
        .to_plugin
        .send(PluginInstruction::RemoveClient(harness.client_id))
        .unwrap();
    harness
        .to_plugin
        .send(PluginInstruction::CliPipe {
            focus_plugin: false,
            is_eof: false,
            pipe_id: "detached-first-pipe".into(),
            name: "hello".into(),
            payload: Some("initial delivery".into()),
            plugin: Some(plugin.clone()),
            args: None,
            configuration: None,
            floating: None,
            pane_id_to_replace: None,
            pane_title: None,
            cwd: None,
            skip_cache: false,
            cli_client_id: harness.client_id,
            source_pane_id: None,
        })
        .unwrap();
    harness.wait_for("received initial delivery");
    harness.write_plugin(
        "detached.ts",
        "export default {render() {return 'reload completed without UI';}};",
    );
    let run =
        yinz_runtime::plugin_location::run_plugin_or_alias_from_url(&plugin, &None, None, None)
            .unwrap();
    harness
        .to_plugin
        .send(PluginInstruction::Reload(
            Some(false),
            None,
            run,
            TabId::new(0),
            Size { rows: 24, cols: 80 },
            None,
        ))
        .unwrap();
    harness.wait_for("reload completed without UI");
}
