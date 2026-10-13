use super::*;
use std::os::unix::fs::PermissionsExt;
struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
const WORKER: &str = r#"#!/usr/bin/python3
import sys, pathlib, http.server, socketserver, json
args=sys.argv[1:]
def arg(name): return args[args.index(name)+1]
token=pathlib.Path(arg('--api-key-file')).read_text()
alias=arg('--alias')
mode=pathlib.Path('fixture-mode').read_text()
if mode != 'cpu-only-build': print('load_tensors: offloaded 0/37 layers to GPU',file=sys.stderr,flush=True)
class Handler(http.server.BaseHTTPRequestHandler):
 def log_message(self,*args): pass
 def reply(self,value):
  if self.headers.get('Authorization') != 'Bearer '+token:
   self.send_response(401);self.end_headers();return
  body=json.dumps(value).encode();self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
 def do_GET(self): self.reply({'data':[{'id':alias}]})
 def do_POST(self):
  self.rfile.read(int(self.headers.get('Content-Length','0')))
  self.reply({'choices':[{'finish_reason':'length' if mode=='incomplete' else 'stop','message':{'content':'{"text":"fixture output"}'}}]})
with socketserver.TCPServer(('127.0.0.1',int(arg('--port'))),Handler) as server: server.serve_forever()
"#;
#[tokio::test]
async fn local_worker_authenticates_and_cleans_up_after_success_or_invalid_output() {
    let root =
        Temp(std::env::temp_dir().join(format!("writer-text-test-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&root.0).unwrap();
    let executable = root.0.join(super::runtime::EXECUTABLE);
    std::fs::write(&executable, crate::test_support::python_fixture(WORKER)).unwrap();
    std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    // cpu-only-build: llama.cpp without any offload-capable device prints no layer report.
    for mode in ["success", "incomplete", "cpu-only-build"] {
        std::fs::write(root.0.join("fixture-mode"), mode).unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            runtime::generate(
                &root.0,
                &root.0,
                &MODELS[0],
                false,
                "fixture instructions",
                "fixture input",
            ),
        )
        .await
        .unwrap();
        if mode != "incomplete" {
            assert_eq!(result.unwrap(), "{\"text\":\"fixture output\"}");
        } else {
            assert!(matches!(
                result.unwrap_err().code,
                ErrorCode::InvalidProviderResponse
            ));
        }
        assert!(std::fs::read_dir(&root.0).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".writer-text-key-")));
    }
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        runtime::generate(&root.0, &root.0, &MODELS[0], true, "fixture", "fixture"),
    )
    .await
    .unwrap();
    assert!(
        matches!(result.unwrap_err().code, ErrorCode::ProviderUnavailable),
        "CPU output must not be reported as GPU execution"
    );
}
