// Offscreen WKWebView renderer for the layout check (macOS). It loads a page at a given size and appearance, runs JS
// for every job, prints the JS result as one JSON line per job and, when asked, saves a PNG snapshot.
// No window is shown and there is no dock icon. Usage: render <jobs.json>
// jobs.json: {"html": "/abs/page.html", "scale": 1, "jobs": [{"js": "...", "theme": "dark", "width": 1180,
//   "height": 760, "fresh": true, "out": "/abs/x.png", "wait": 100}]}
// A job with "fresh" reloads the page first (the runner sets it when the size or the theme changes).
import AppKit
import WebKit

struct Job: Decodable {
    let js: String
    let theme: String
    let width: Double
    let height: Double
    let fresh: Bool?
    let out: String?
    let wait: Int?
}
struct Jobs: Decodable { let html: String; let scale: Double?; let jobs: [Job] }

final class Runner: NSObject, WKNavigationDelegate {
    let cfg: Jobs
    var i = 0
    var web: WKWebView!
    var win: NSWindow!
    init(_ c: Jobs) { cfg = c }

    func start() {
        let rect = NSRect(x: 0, y: 0, width: 1180, height: 760)
        win = NSWindow(contentRect: rect, styleMask: [.borderless], backing: .buffered, defer: false)
        win.setFrameOrigin(NSPoint(x: -20000, y: -20000))
        web = WKWebView(frame: rect, configuration: WKWebViewConfiguration())
        web.navigationDelegate = self
        win.contentView = web
        win.orderFront(nil)
        next()
    }

    func next() {
        guard i < cfg.jobs.count else { exit(0) }
        arm()
        let job = cfg.jobs[i]
        let ap = NSAppearance(named: job.theme == "light" ? .aqua : .darkAqua)
        win.appearance = ap
        web.appearance = ap
        win.setContentSize(NSSize(width: job.width, height: job.height))
        web.frame = NSRect(x: 0, y: 0, width: job.width, height: job.height)
        if job.fresh ?? (i == 0) {
            let base = URL(fileURLWithPath: cfg.html)
            web.loadFileURL(base, allowingReadAccessTo: base.deletingLastPathComponent())
        } else {
            run()
        }
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { self.run() }
    }

    var finished = -1
    var watchdog: DispatchWorkItem?

    // every result is one JSON line; errors are JSON-encoded, so quotes and newlines in a message cannot break it
    func errorLine(_ idx: Int, _ msg: String) -> String {
        let data = try? JSONSerialization.data(withJSONObject: ["i": idx, "error": msg])
        return String(data: data ?? Data(), encoding: .utf8) ?? "{\"i\":\(idx),\"error\":\"unprintable\"}"
    }

    func finish(_ line: String) {
        let idx = i
        if finished == idx { return }
        finished = idx
        watchdog?.cancel()
        print(line)
        fflush(stdout)
        i += 1
        next()
    }

    func arm() {
        watchdog?.cancel()
        let idx = i
        let w = DispatchWorkItem { [weak self] in self?.finish(self?.errorLine(idx, "timeout: the page did not answer in 30 s") ?? "") }
        watchdog = w
        DispatchQueue.main.asyncAfter(deadline: .now() + 30, execute: w)
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        finish(errorLine(i, "navigation failed: \(error.localizedDescription)"))
    }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        finish(errorLine(i, "navigation failed: \(error.localizedDescription)"))
    }

    func run() {
        let job = cfg.jobs[i]
        let idx = i
        web.evaluateJavaScript(job.js) { result, error in
            if let error {
                self.finish(self.errorLine(idx, "\(error)"))
                return
            }
            let r = (result as? String) ?? "null"
            guard let out = job.out else { self.finish("{\"i\":\(idx),\"r\":\(r)}"); return }
            DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(job.wait ?? 120)) {
                let sc = WKSnapshotConfiguration()
                sc.snapshotWidth = NSNumber(value: job.width * (self.cfg.scale ?? 1))
                self.web.takeSnapshot(with: sc) { img, err in
                    if let img, let tiff = img.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
                       let png = rep.representation(using: .png, properties: [:]) {
                        try? png.write(to: URL(fileURLWithPath: out))
                    }
                    self.finish("{\"i\":\(idx),\"r\":\(r)}")
                }
            }
        }
    }
}

let data = try! Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
let cfg = try! JSONDecoder().decode(Jobs.self, from: data)
let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
let runner = Runner(cfg)
runner.start()
app.run()
