const http = require('http');

const OLLAMA_URL = 'http://10.128.13.78:11434';

http.createServer((req, res) => {
    // 强制修改路径，去掉 ?beta=true
    const url = new URL(req.url, OLLAMA_URL);
    const options = {
        hostname: url.hostname,
        port: url.port,
        path: url.pathname, // 这里去掉了 searchParams (即 beta=true)
        method: req.method,
        headers: { ...req.headers }
    };

    // 删除让 Ollama 崩溃的 Header
    delete options.headers['anthropic-beta'];
    delete options.headers['host'];

    const proxyReq = http.request(options, (proxyRes) => {
        res.writeHead(proxyRes.statusCode, proxyRes.headers);
        proxyRes.pipe(res);
    });

    req.pipe(proxyReq);
}).listen(11435, () => {
    console.log('Ollama Claude-Fix Proxy running on http://localhost:11435');
});
