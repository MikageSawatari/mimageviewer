use super::*;
use std::sync::atomic::AtomicUsize;

#[test]
fn help_tools_answer_all_protocols_without_backend_when_unreachable_or_disabled() {
    struct NoCore(AtomicUsize);
    impl Backend for NoCore {
        fn call(
            &self,
            _: Command,
            _: &ClientIdentity,
            _: RequestKind,
            _: Arc<CallControl>,
        ) -> BackendResult {
            panic!(
                "Help opened the core backend, availability={}",
                self.0.load(Ordering::Relaxed)
            );
        }
    }
    for version in VERSIONS {
        let backend = Arc::new(NoCore(AtomicUsize::new(0)));
        let mut h = Harness::new(backend.clone());
        if version != MODERN {
            h.send(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}));
            h.receive();
            h.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        }
        // 0: not running; 1: integration disabled. Neither is queried.
        for availability in [0, 1] {
            backend.0.store(availability, Ordering::Relaxed);
            for (name, args) in [
                ("get_help", json!({})),
                ("get_help", json!({"id":"index.html"})),
                ("get_help", json!({"id":"sound/index.html"})),
                ("get_help", json!({"id":"search.html#ctrl-g"})),
                ("search_help", json!({"query":"検索 ctrl+g","limit":30})),
            ] {
                let tool = schema::tools()
                    .into_iter()
                    .find(|t| t["name"] == name)
                    .unwrap();
                assert!(accepts_schema(&tool["inputSchema"], &args));
                h.send(version_request(
                    version,
                    1,
                    "tools/call",
                    json!({"name":name,"arguments":args}),
                ));
                let reply = h.receive();
                assert_eq!(reply["id"], 1);
                assert_eq!(reply["result"]["isError"], false, "{reply}");
                let content = &reply["result"]["structuredContent"];
                assert_eq!(content["app_version"], env!("CARGO_PKG_VERSION"));
                assert!(accepts_schema(&tool["outputSchema"], content));
                assert_eq!(
                    serde_json::from_str::<Value>(
                        reply["result"]["content"][0]["text"].as_str().unwrap()
                    )
                    .unwrap(),
                    *content
                );
            }
        }
        h.send(version_request(
            version,
            2,
            "tools/call",
            json!({"name":"search_help","arguments":{"query":" "}}),
        ));
        assert_eq!(h.receive()["error"]["code"], -32602);
        h.send(version_request(
            version,
            3,
            "tools/call",
            json!({"name":"get_help","arguments":{"id":"unknown"}}),
        ));
        let reply = h.receive();
        assert_eq!(reply["result"]["isError"], true);
        assert!(accepts_schema(
            &schema::tools()
                .into_iter()
                .find(|t| t["name"] == "get_help")
                .unwrap()["outputSchema"],
            &reply["result"]["structuredContent"]
        ));
        h.close();
    }
}
