// Loads the installed production assembly, without recompiling or patching it.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Management;
using System.Net;
using System.Reflection;
using System.Runtime.Remoting.Messaging;
using System.Runtime.Remoting.Proxies;
using System.Security.Cryptography;
using System.Threading;
using System.Threading.Tasks;
using System.Text;
using System.Web.Script.Serialization;
using System.Windows.Forms;
using Microsoft.Web.WebView2.Core;
using Microsoft.Web.WebView2.WinForms;

static class Installed {
    const BindingFlags Flags = BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic;
    static readonly JavaScriptSerializer Json = new JavaScriptSerializer { MaxJsonLength = 4000000 };
    static Assembly product;
    static Form app;
    static WebView2 browser;
    static string package, root, script, clip, receipt;
    static string model, language;
    static int speechMs;
    static readonly Dictionary<string, object> evidence = new Dictionary<string, object>();
    static readonly List<string> passed = new List<string>();
    static readonly List<string> failures = new List<string>();
    static object Field(object target, string name) { return target.GetType().GetField(name, Flags).GetValue(target); }
    static object Call(object target, string name, params object[] args) { return target.GetType().GetMethod(name, Flags).Invoke(target, args); }
    static async Task Bounded(Task task, string label, int seconds = 45) {
        if (await Task.WhenAny(task, Task.Delay(seconds * 1000)) != task) throw new Exception("Operation deadline: " + label);
        await task;
    }
    static async Task Invoke(object target, string name, params object[] args) { await Bounded((Task)Call(target, name, args), name); }
    static void Check(bool value, string label) { if (!value) throw new Exception(label); passed.Add(label); Console.WriteLine("PASS " + label); evidence["status"]="running"; evidence["passed"]=passed; File.WriteAllText(receipt, Json.Serialize(evidence)); }
    static string Hash(byte[] bytes) { using (var sha = SHA256.Create()) return BitConverter.ToString(sha.ComputeHash(bytes)).Replace("-", "").ToLowerInvariant(); }
    static async Task<object> Eval(string expression) {
        var evaluation = browser.CoreWebView2.CallDevToolsProtocolMethodAsync("Runtime.evaluate", Json.Serialize(new { expression = "(async()=>{return " + expression + ";})()", awaitPromise = true, returnByValue = true }));
        await Bounded(evaluation, "evaluate " + expression);
        string response = await evaluation;
        var value = Json.Deserialize<Dictionary<string, object>>(response);
        if (value.ContainsKey("exceptionDetails")) throw new Exception("Evaluation failed: " + expression);
        var result = (Dictionary<string, object>)value["result"];
        return result.ContainsKey("value") ? result["value"] : null;
    }
    static async Task Until(string expression, int seconds = 60) {
        var watch = Stopwatch.StartNew();
        while (watch.Elapsed.TotalSeconds < seconds) { if (Object.Equals(await Eval(expression), true)) return; await Task.Delay(150); }
        evidence["timeout_snapshot"] = await Eval("window.writerDesktopSnapshot?.()");
        evidence["timeout_ui"] = await Eval("document.body.innerText");
        throw new Exception("Deadline: " + expression);
    }
    static async Task Open() {
        app = (Form)Activator.CreateInstance(product.GetType("MetrixWriter.Composition.IntegratedWindow", true), Flags, null,
            new object[] { package, root, SynchronizationContext.Current, true }, null);
        var handle = app.Handle;
        browser = (WebView2)Field(app, "Browser");
        Func<CoreWebView2, Task> configure = async core => {
            await core.AddScriptToExecuteOnDocumentCreatedAsync("window.acceptanceConfig=" + Json.Serialize(new { model, language }) + ";window.acceptanceClip=" + Json.Serialize(Convert.ToBase64String(File.ReadAllBytes(clip))) + ";\n" + File.ReadAllText(script));
        };
        await Invoke(app, "Initialize", "--autoplay-policy=no-user-gesture-required", configure);
        Check(!app.Visible, "installed window stays hidden");
        await Until("!!document.querySelector('#setup-heading') && !!window.writerDesktopSnapshot");
    }
    static async Task Close() {
        if (app == null) return;
        await Invoke(app, "Quit");
        var backend = Field(app, "Backend");
        var snapshot = backend.GetType().GetProperty("Snapshot").GetValue(backend, null);
        Check(Convert.ToString(snapshot.GetType().GetProperty("Phase").GetValue(snapshot, null)) == "Stopped", "production Quit stops owned backend");
        app.Dispose(); app = null;
    }
    static async Task Command(string action, string id) {
        evidence["last_command"] = action; File.WriteAllText(receipt, Json.Serialize(evidence));
        var command = Json.Deserialize(Json.Serialize(new { version = 1, kind = "command", epoch = await Eval("writerDesktopSnapshot().epoch"), command_id = Guid.NewGuid().ToString(), action, session_id = id }), product.GetType("MetrixWriter.Contracts.SessionCommand", true));
        var pending = (Task)Call(Field(app, "Session"), "DispatchAsync", command);
        await Bounded(pending, "command " + action);
        var ack = pending.GetType().GetProperty("Result").GetValue(pending, null);
        if (Convert.ToString(ack.GetType().GetProperty("outcome").GetValue(ack, null)) != "accepted") throw new Exception("Command rejected: " + action);
    }
    static async Task Capture(string id, int milliseconds, string action) {
        await Command("start", id);
        await Until("writerDesktopSnapshot().state === 'capturing'");
        await Task.Delay(milliseconds);
        await Command(action, id);
        evidence["after_stop_snapshot"] = await Eval("writerDesktopSnapshot()"); File.WriteAllText(receipt, Json.Serialize(evidence));
    }
    static Process ApiProcess() { return (Process)Field(Field(app, "Backend"), "child"); }
    static Process Worker() {
        int parent = ApiProcess().Id;
        using (var query = new ManagementObjectSearcher("SELECT ProcessId, ExecutablePath FROM Win32_Process WHERE ParentProcessId=" + parent + " AND Name='whisper-server.exe'"))
        foreach (ManagementObject row in query.Get()) {
            string path = Convert.ToString(row["ExecutablePath"]);
            if (!path.StartsWith(package + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase)) throw new Exception("Worker outside installed package");
            return Process.GetProcessById(Convert.ToInt32(row["ProcessId"]));
        }
        throw new Exception("Owned worker absent");
    }
    static async Task Ready() {
        await Until("(await acceptance.api('setup')).runtime.worker.state === 'ready'", 180);
        // Worker setup readiness precedes the provider adapter's bounded health probe.
        // Record only once the mounted product UI permits transcription.
        await Until("document.querySelector('.model-settings [role=status]')?.textContent === 'Local runtime and loaded model passed readiness checks.'", 60);
    }
    static string[] Jobs() { return Directory.GetFiles(Path.Combine(root, "Data", "jobs"), "*.json"); }
    static async Task MultipartRecovery() {
        string id=Guid.NewGuid().ToString(), boundary="acceptance-"+Guid.NewGuid().ToString("N");
        byte[] audio=File.ReadAllBytes(clip);
        if(audio.Length<44 || Encoding.ASCII.GetString(audio,0,4)!="RIFF" || Encoding.ASCII.GetString(audio,8,4)!="WAVE") throw new Exception("ControlsOnly requires a WAV input");
        string metadata=Json.Serialize(new { api_version="1", data=new { id, created_at_ms=DateTimeOffset.UtcNow.ToUnixTimeMilliseconds(), mime_type="audio/wav", byte_length=audio.Length, duration_ms=0, input_language=language, capture_state="complete" } });
        byte[] prefix=Encoding.UTF8.GetBytes("--"+boundary+"\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n"+metadata+"\r\n--"+boundary+"\r\nContent-Disposition: form-data; name=\"audio\"; filename=\"speech.wav\"\r\nContent-Type: audio/wav\r\n\r\n");
        var backend=Field(app,"Backend");
        var interrupted=(HttpWebRequest)Call(backend,"Request","/api/v1/recordings","POST");
        interrupted.SendChunked=true; interrupted.AllowWriteStreamBuffering=false; interrupted.ContentType="multipart/form-data; boundary="+boundary;
        var opening=interrupted.GetRequestStreamAsync(); await Bounded(opening,"partial upload stream");
        var stream=await opening;
        try {
            await Bounded(stream.WriteAsync(prefix,0,prefix.Length),"partial metadata");
            await Bounded(stream.WriteAsync(audio,0,audio.Length/2),"partial audio"); await Bounded(stream.FlushAsync(),"partial flush");
            string partial=Path.Combine(root,"Data","recordings",id+".uploading","audio");
            var deadline=Stopwatch.StartNew();
            while((!File.Exists(partial) || new FileInfo(partial).Length==0) && deadline.Elapsed.TotalSeconds<10) await Task.Delay(50);
            Check(File.Exists(partial) && new FileInfo(partial).Length>0 && new FileInfo(partial).Length<audio.Length,"partial audio reaches installed server before interruption");
            evidence["interrupted_upload_received_bytes"]=new FileInfo(partial).Length;
        }
        finally { interrupted.Abort(); try { stream.Dispose(); } catch(IOException) {} }
        await Until("(await acceptance.api('recordings/"+id+"')).audio.state==='uploading'");
        var retry=(HttpWebRequest)Call(backend,"Request","/api/v1/recordings","POST");
        retry.SendChunked=true; retry.ContentType="multipart/form-data; boundary="+boundary;
        opening=retry.GetRequestStreamAsync(); await Bounded(opening,"retry upload stream");
        using(var body=await opening) {
            await Bounded(body.WriteAsync(prefix,0,prefix.Length),"retry metadata"); await Bounded(body.WriteAsync(audio,0,audio.Length),"retry audio");
            byte[] suffix=Encoding.UTF8.GetBytes("\r\n--"+boundary+"--\r\n"); await Bounded(body.WriteAsync(suffix,0,suffix.Length),"retry footer");
        }
        var response=retry.GetResponseAsync(); await Bounded(response,"retry upload response");
        using(var accepted=(HttpWebResponse)await response) Check((int)accepted.StatusCode<300,"interrupted multipart upload retries with the same recording ID");
        Check(Hash(File.ReadAllBytes(Path.Combine(root,"Data","recordings",id,"audio")))==Hash(audio),"retried upload retains exact saved input bytes");
        Check(Object.Equals(await Eval("document.querySelectorAll('.recording-row').length===2 && acceptance.submissions.length===0"),true),"interrupted upload leaves previous browser recordings intact");
    }
    static object New(string type, params object[] args) { return Activator.CreateInstance(product.GetType(type, true), Flags, null, args, null); }
    static async Task Edge(object binding, string gesture, int mode, int edge) {
        var value = Json.Deserialize(Json.Serialize(new { GestureId=gesture, Mode=mode, Edge=edge, Repeat=false }), product.GetType("MetrixWriter.Contracts.ShortcutEdge", true));
        await Invoke(binding, "HandleAsync", value);
    }
    sealed class Keyboard : RealProxy {
        readonly Type contract;
        internal Keyboard(Type type) : base(type) { contract=type; }
        public override IMessage Invoke(IMessage message) {
            var call=(IMethodCallMessage)message;
            object value=null;
            if(call.MethodName=="HeldKeys") value=new int[0];
            else if(call.MethodName=="GetType") value=contract;
            else if(call.MethodName=="ToString") value="inert acceptance keyboard";
            else if(call.MethodName=="GetHashCode") value=0;
            else if(call.MethodName!="Dispose" && !call.MethodName.StartsWith("add_") && !call.MethodName.StartsWith("remove_"))
                return new ReturnMessage(new Exception("Native keyboard access is forbidden in acceptance: " + call.MethodName),call);
            return new ReturnMessage(value,null,0,call.LogicalCallContext,call);
        }
    }
    static async Task Controls() {
        await Open();
        // Local transparent proxy implements the installed internal interface.
        // It opens no remoting channel and constructs no native keyboard source.
        var source = new Keyboard(product.GetType("MetrixWriter.Input.IKeyboardSource",true)).GetTransparentProxy();
        var input = New("MetrixWriter.Input.NativeShortcutInput", source);
        var binding = New("MetrixWriter.Input.ShortcutSessionBinding", input, Field(app,"Session"));
        try {
            Check(!(bool)Field(input,"started"), "installed input uses an inert injected keyboard source");
            string hold=Guid.NewGuid().ToString();
            await Edge(binding,hold,0,0); await Until("writerDesktopSnapshot().state==='capturing'");
            await Task.Delay(1600); await Edge(binding,hold,0,1);
            await Until("writerDesktopSnapshot().state==='saved' && writerDesktopSnapshot().audio_saved");
            Check(Object.Equals(await Eval("acceptance.streams===1 && acceptance.submissions.length===0"),true), "injected hold release saves one capture without a configured provider");
            string toggle=Guid.NewGuid().ToString();
            await Edge(binding,toggle,1,0); await Until("writerDesktopSnapshot().state==='capturing'");
            await Edge(binding,toggle,1,0); await Edge(binding,toggle,1,1);
            Check(Object.Equals(await Eval("writerDesktopSnapshot().state==='capturing' && acceptance.streams===2"),true), "toggle ignores duplicate down and release");
            await Task.Delay(1600); await Edge(binding,Guid.NewGuid().ToString(),1,0);
            await Until("writerDesktopSnapshot().state==='saved' && writerDesktopSnapshot().audio_saved");
            Check(true,"second injected toggle stops and retains audio");
            await Eval("acceptance.delayMedia=true");
            string pending=Guid.NewGuid().ToString();
            await Edge(binding,pending,0,0); await Until("typeof acceptance.releaseMedia==='function'");
            await Bounded(Task.WhenAll(Edge(binding,pending,0,1),Edge(binding,pending,0,2)),"release/cancel race");
            await Eval("acceptance.releaseMedia()");
            await Until("writerDesktopSnapshot().state==='canceled' && acceptance.streams===3");
            Check(Object.Equals(await Eval("acceptance.submissions.length===0 && document.querySelectorAll('.recording-row').length===2"),true), "release/cancel during pending capture leaves prior recordings and no submission");
            Check(!(bool)Field(input,"started") && !app.Visible,"injected native controls never register host input or show a window");
            // Pure presentation fixture through installed code: no overlay window.
            long now=1000;
            var overlay=New("MetrixWriter.Overlay.OverlayModel",new Func<long>(()=>now),null);
            var snapshot=Json.Deserialize<Dictionary<string,object>>(Json.Serialize(await Eval("writerDesktopSnapshot()")));
            snapshot["state"]="transcribing"; snapshot["job_id"]=Guid.NewGuid().ToString(); snapshot["audio_saved"]=true; snapshot["revision"]=100;
            var type=product.GetType("MetrixWriter.Contracts.SessionSnapshot",true);
            overlay.GetType().GetMethod("Observe",Flags,null,new[]{type},null).Invoke(overlay,new[]{Json.Deserialize(Json.Serialize(snapshot),type)});
            var view=overlay.GetType().GetProperty("View",Flags).GetValue(overlay,null);
            double phase=(double)Field(view,"IndeterminatePhase"); now+=200; Call(overlay,"Tick");
            view=overlay.GetType().GetProperty("View",Flags).GetValue(overlay,null);
            Check(Convert.ToString(Field(view,"Phase"))=="Transcribing" && (bool)Field(view,"Indeterminate") && (double)Field(view,"IndeterminatePhase")!=phase,"installed overlay model advances indeterminate transcribing phase");
            overlay.GetType().GetProperty("ReducedMotion",Flags).SetValue(overlay,true,null); now+=200; Call(overlay,"Tick");
            view=overlay.GetType().GetProperty("View",Flags).GetValue(overlay,null);
            Check((double)Field(view,"IndeterminatePhase")==0,"installed overlay model honors reduced motion");
            await MultipartRecovery();
            await Invoke(binding,"ShutdownAsync",CancellationToken.None);
        } finally { ((IDisposable)binding).Dispose(); }
        await Close();
    }
    static async Task Run(string weights, bool longCapture) {
        await Open();
        Check(Object.Equals(await Eval("(await acceptance.api('setup')).first_run"), true), "fresh private app data reports first run");
        Check(Object.Equals(await Eval("(await acceptance.api('setup')).cloud.every(c => c.credential === 'absent')"), true), "no inherited cloud credentials");
        await Eval("acceptance.select()");
        await Until("(await acceptance.api('setup')).runtime.worker.state === 'missing_weights'");
        Check(true, "missing selected weights reported without implicit download");
        var importer = product.GetType("MetrixWriter.TranscriptionSetup.WeightsImport", true);
        importer.GetMethod("Import", Flags).Invoke(null, new object[] { Path.Combine(root, "Models"), model, weights });
        await Eval("acceptance.api('setup/models/" + model + "/verify','POST')");
        await Until("(await acceptance.api('setup')).models.find(m=>m.id===" + Json.Serialize(model) + ").weights.state==='installed'", 180);
        await Eval("acceptance.api('setup/runtime/retry','POST')");
        await Ready();
        evidence["worker"] = await Eval("(await acceptance.api('setup')).runtime.worker");
        evidence["weights"] = await Eval("(await acceptance.api('setup')).models.find(m=>m.id===" + Json.Serialize(model) + ").weights");
        Check(Object.Equals(await Eval("(await acceptance.api('setup')).runtime.worker.backend"), "vulkan"), "installed runtime confirms Vulkan inference");
        await Close(); await Open(); await Ready();
        Check(Object.Equals(await Eval("(await acceptance.api('settings')).transcription.input_language"), language), "relaunch restores selected speech language");
        await Task.Delay(4000);
        string id = Guid.NewGuid().ToString();
        var watch = Stopwatch.StartNew();
        evidence["before_capture_ui"] = await Eval("document.body.innerText");
        await Capture(id, speechMs, "stop");
        await Until("!!document.querySelector('textarea[aria-label=\"Original transcript\"]')?.value", 180);
        evidence["capture_to_text_ms"] = watch.ElapsedMilliseconds;
        string raw = Convert.ToString(await Eval("document.querySelector('textarea[aria-label=\"Original transcript\"]').value"));
        evidence["raw_text"] = raw; evidence["raw_sha256"] = Hash(System.Text.Encoding.UTF8.GetBytes(raw));
        evidence["source_audio_sha256"] = Hash(File.ReadAllBytes(clip));
        string audio = Path.Combine(root, "Data", "recordings", id, "audio");
        string hash = Hash(File.ReadAllBytes(audio)); evidence["retained_audio_sha256"] = hash;
        Check(raw.Length > 0, "public saved speech produces real original text");
        Check(Object.Equals(await Eval("acceptance.streams===1 && acceptance.uploads===1 && acceptance.submissions.length===1"), true), "one stream upload and submission");
        evidence["meter"] = await Eval("({count:acceptance.levels.length,min:acceptance.levels.length?Math.min(...acceptance.levels):null,max:acceptance.levels.length?Math.max(...acceptance.levels):null})");
        if (Object.Equals(await Eval("acceptance.levels.some(v=>v>0.001) && acceptance.levels.some(v=>v<0.00001)"), true)) Check(true, "meter distinguishes saved speech and silence");
        else { failures.Add("meter did not distinguish saved speech and silence in hidden instrumentation"); evidence["failures"]=failures; }
        await Eval("[...document.querySelectorAll('button')].find(b=>b.textContent==='Copy original').click()");
        await Until("acceptance.copied === document.querySelector('textarea[aria-label=\"Original transcript\"]').value");
        Check(true, "copy original reaches isolated clipboard sink");
        using (var worker = Worker()) { evidence["killed_owned_worker_pid"] = worker.Id; worker.Kill(); worker.WaitForExit(5000); }
        await Until("(await acceptance.api('setup')).runtime.worker.restarts > 0", 180); await Ready();
        int before = Jobs().Length;
        await Command("retry", id);
        await Until("acceptance.submissions.length===2");
        await Until("writerDesktopSnapshot().state==='success'", 180);
        Check(Jobs().Length == before + 1 && Hash(File.ReadAllBytes(audio)) == hash, "worker crash retry retains identical audio and one new attempt");
        Check(Object.Equals(await Eval("acceptance.streams===1 && acceptance.uploads===1"), true), "retry does not recapture or upload");
        string firstJob = Jobs()[0]; string firstJobHash = Hash(File.ReadAllBytes(firstJob));
        var backend = Field(app, "Backend");
        var api = ApiProcess(); evidence["killed_owned_backend_pid"] = api.Id; api.Kill(); api.WaitForExit(5000);
        await Until("(await fetch('/api/v1/health')).status===503");
        Check(Object.Equals(await Eval("!!document.querySelector('textarea[aria-label=\"Original transcript\"]')?.value"), true), "backend crash leaves original text visible");
        await Invoke(backend, "ReconnectAsync", CancellationToken.None); await Ready();
        Check(Hash(File.ReadAllBytes(firstJob)) == firstJobHash && Hash(File.ReadAllBytes(audio)) == hash, "backend reconnect preserves completed job and audio bytes");
        await Eval("acceptance.lose='/api/v1/transcriptions'");
        before = Jobs().Length;
        await Command("retry", id); await Until("writerDesktopSnapshot().state==='error'");
        await Command("retry", id); await Until("writerDesktopSnapshot().state==='success'", 180);
        Check(Object.Equals(await Eval("acceptance.submissions.at(-1).idempotency_key===acceptance.submissions.at(-2).idempotency_key"), true) && Jobs().Length == before + 1,
            "lost submission response reuses persisted key and creates one job");
        before = Jobs().Length - 1;
        evidence["state_trace"] = await Eval("acceptance.states");
        await Close(); await Open(); await Ready();
        await Until("!!document.querySelector('textarea[aria-label=\"Original transcript\"]')?.value");
        Check(Hash(File.ReadAllBytes(audio)) == hash && Jobs().Length == before + 1, "app restart retains audio and attempts without resubmission");
        await Eval("acceptance.deny=true");
        await Command("start", Guid.NewGuid().ToString());
        await Until("writerDesktopSnapshot().state==='error'");
        Check(Hash(File.ReadAllBytes(audio)) == hash, "injected permission denial preserves prior audio");
        await Eval("(acceptance.deny=false, acceptance.storageFail=true)");
        await Command("start", Guid.NewGuid().ToString());
        await Until("writerDesktopSnapshot().state==='error'");
        Check(Object.Equals(await Eval("acceptance.storageFailures===1"), true) && Hash(File.ReadAllBytes(audio)) == hash && Hash(File.ReadAllBytes(firstJob)) == firstJobHash, "recording-store failure preserves previous audio and raw job");
        await Eval("acceptance.storageFail=false");
        await Eval("(acceptance.deny=false, acceptance.silence=true)");
        string canceled = Guid.NewGuid().ToString();
        await Capture(canceled, 1600, "cancel");
        await Until("writerDesktopSnapshot().state!=='capturing' && writerDesktopSnapshot().state!=='saving'");
        Check(Object.Equals(await Eval("writerDesktopSnapshot().audio_saved"), true), "cancel retains committed synthetic audio");
        if (longCapture) {
            // Keep the injected source producing frames for the entire interval.
            // Wall-clock metadata alone cannot prove encoded audio duration.
            await Eval("(acceptance.silence=false, acceptance.loop=true)");
            string longId = Guid.NewGuid().ToString();
            watch.Restart();
            await Capture(longId, 305000, "cancel");
            await Until("writerDesktopSnapshot().audio_saved && writerDesktopSnapshot().state!=='saving'");
            evidence["long_capture_elapsed_ms"] = watch.ElapsedMilliseconds;
            Check(watch.ElapsedMilliseconds > 300000, "over five minutes real elapsed capture saved");
            string savedExpression = "acceptance.saved(" + Json.Serialize(longId) + ")";
            string longSaved = Json.Serialize(await Eval(savedExpression)); evidence["long_recording"] = Json.DeserializeObject(longSaved);
            Check(Object.Equals(await Eval("(await " + savedExpression + ").metadata.duration_ms > 300000 && (await " + savedExpression + ").decoded_duration_ms > 300000 && (await " + savedExpression + ").bytes > 0"), true), "long recording has over five minutes of decoded audio");
            await Close(); await Open();
            Check(Json.Serialize(await Eval(savedExpression)) == longSaved, "long recording metadata and audio hash survive relaunch");
        }
        await Close();
    }
    [STAThread] static int Main(string[] args) {
        if (args.Length != 10) return 2;
        package=args[0]; root=args[1]; script=args[2]; clip=args[3]; receipt=args[5];
        model=args[7]; language=args[8]; speechMs=Int32.Parse(args[9])*1000;
        if ((model!="ivrit-turbo" && model!="whisper-large-v3-turbo") || (language!="he" && language!="en" && language!="mixed") || (model=="ivrit-turbo" && language!="he") || speechMs<1000 || speechMs>600000) return 2;
        evidence["mode"]=args[6]; evidence["model"]=model; evidence["language"]=language; evidence["requested_speech_capture_ms"]=speechMs;
        product=Assembly.LoadFrom(Path.Combine(package,"MetrixWriter.exe"));
        int code=0;
        Application.EnableVisualStyles(); Application.SetCompatibleTextRenderingDefault(false);
        EventHandler start=null;
        start=async (s,e)=> {
            Application.Idle-=start;
            SynchronizationContext.SetSynchronizationContext(new WindowsFormsSynchronizationContext());
            try { if(args[6]=="controls") await Controls(); else await Run(args[4], args[6]=="long"); evidence["status"]="passed"; }
            catch(Exception error) { evidence["status"]="failed"; evidence["failure"]=error.Message; Console.Error.WriteLine(error.Message); code=1; }
            try { await Close(); } catch { evidence["cleanup_failed"]=true; code=1; }
            if(app!=null) { try { await Invoke(app,"Shutdown"); app.Dispose(); } catch { code=1; } }
            if(failures.Count>0) code=1;
            evidence["status"]=code==0 ? "passed" : "failed";
            evidence["passed"]=passed; evidence["utc"]=DateTime.UtcNow.ToString("o");
            File.WriteAllText(receipt,Json.Serialize(evidence)); Application.ExitThread();
        };
        Application.Idle+=start; Application.Run(new ApplicationContext()); return code;
    }
}
