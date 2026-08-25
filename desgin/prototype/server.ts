import express from "express";
import path from "path";
import { createServer as createViteServer } from "vite";
import { GoogleGenAI } from "@google/genai";

async function startServer() {
  const app = express();
  const PORT = 3001;

  app.use(express.json({ limit: "10mb" }));

  // Shared Gemini client (Lazy or conditional)
  const getAiClient = () => {
    const apiKey = process.env.GEMINI_API_KEY;
    if (!apiKey) return null;
    return new GoogleGenAI({
      apiKey,
      httpOptions: {
        headers: {
          'User-Agent': 'aistudio-build',
        },
      },
    });
  };

  // Health check API
  app.get("/api/health", (_req, res) => {
    res.json({ status: "ok", app: "WinSSH Pro Backend", time: new Date().toISOString() });
  });

  // AI Terminal Copilot API
  app.post("/api/ai/copilot", async (req, res) => {
    try {
      const { prompt, type, context } = req.body;
      const ai = getAiClient();

      if (!prompt) {
        return res.status(400).json({ error: "Missing prompt" });
      }

      if (!ai) {
        // High quality offline fallback generator when no API key is provided
        return res.json({
          success: true,
          source: "offline-engine",
          result: generateOfflineBashAssistance(prompt, type, context),
        });
      }

      let systemInstruction = "You are WinSSH Pro Copilot, an elite Linux DevOps and SSH Terminal Assistant. ";
      if (type === "generate_command") {
        systemInstruction += "Generate concise, safe, production-grade Linux Bash/Shell commands for the user's natural language request. Provide the primary command, a brief explanation, safety warning if destructive, and flags breakdown.";
      } else if (type === "explain_command") {
        systemInstruction += "Explain what the provided shell command does, breaking down each argument and flag, along with potential side effects.";
      } else if (type === "diagnose_error") {
        systemInstruction += "Analyze the terminal error log and provide the exact root cause and 2-3 step actionable fix commands.";
      } else if (type === "optimize_script") {
        systemInstruction += "Review and optimize the provided shell script for security, performance, idempotency, and best practices.";
      }

      const response = await ai.models.generateContent({
        model: "gemini-3.7-flash",
        contents: `Task Type: ${type || "general"}\nUser Input: ${prompt}\nServer OS Context: ${context?.os || "Linux (Ubuntu/Debian)"}\nCurrent Working Directory: ${context?.cwd || "/root"}\n`,
        config: {
          systemInstruction,
          temperature: 0.2,
        },
      });

      res.json({
        success: true,
        source: "gemini-3.7-flash",
        result: response.text || "No response generated.",
      });
    } catch (error: any) {
      console.error("AI Copilot Error:", error);
      // Graceful fallback to offline smart engine
      const { prompt, type, context } = req.body || {};
      res.json({
        success: true,
        source: "offline-fallback",
        result: generateOfflineBashAssistance(prompt || "", type || "generate_command", context),
      });
    }
  });

  // Mock server ping & latency telemetry tester
  app.post("/api/network/ping-check", (req, res) => {
    const { host } = req.body;
    const latency = Math.floor(Math.random() * 35) + 12; // 12-47ms
    const packetLoss = 0;
    res.json({
      host: host || "127.0.0.1",
      latencyMs: latency,
      packetLoss,
      status: "online",
      sshPortOpen: true,
      banner: "SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.4",
    });
  });

  // Vite middleware for development vs static production serve
  if (process.env.NODE_ENV !== "production") {
    const vite = await createViteServer({
      server: { middlewareMode: true },
      appType: "spa",
    });
    app.use(vite.middlewares);
  } else {
    const distPath = path.join(process.cwd(), "dist");
    app.use(express.static(distPath));
    app.get("*", (_req, res) => {
      res.sendFile(path.join(distPath, "index.html"));
    });
  }

  app.listen(PORT, "0.0.0.0", () => {
    console.log(`[WinSSH Pro] Server running on http://0.0.0.0:${PORT}`);
  });
}

function generateOfflineBashAssistance(prompt: string, type: string, _context?: any): string {
  const p = prompt.toLowerCase();

  if (type === "explain_command") {
    return `### 📖 Command Explanation
\`\`\`bash
${prompt}
\`\`\`
- **Core Action**: Executes command pipeline with elevated privileges if applicable.
- **Safety Rating**: Standard
- **Key Flags**:
  - Validates exit codes and output streams.
- **Recommended Usage**: Run in staging first if impacting active system services.`;
  }

  if (type === "diagnose_error") {
    return `### 🔍 Error Diagnosis & Fix
**Analysis**: The error indicates a permission issue, missing socket, or service port conflict.

**Recommended Solution**:
\`\`\`bash
# 1. Check service and port status
sudo netstat -tulpn | grep -E ':(80|443|22)'
# 2. Check systemd daemon logs
sudo journalctl -u $(systemctl list-unit-files | grep failed | awk '{print $1}') -e --no-pager -n 25
# 3. Restart failed unit cleanly
sudo systemctl daemon-reload && sudo systemctl restart nginx
\`\`\``;
  }

  // Generate command heuristics
  if (p.includes("docker") || p.includes("container")) {
    return `### 🐳 Docker Management Command
\`\`\`bash
# List all containers with resource usage
docker ps -a --format "table {{.ID}}\\t{{.Names}}\\t{{.Status}}\\t{{.Ports}}"

# Remove all unused dangling images & stopped containers
docker system prune -af --volumes
\`\`\`
**Explanation**: Cleans up disk space by removing unused Docker layers and containers safely.`;
  }

  if (p.includes("port") || p.includes("kill") || p.includes("listen")) {
    return `### 🔌 Port & Process Inspection
\`\`\`bash
# Find process occupying specific port (e.g. 8080 or 3000)
sudo lsof -i :8080 || sudo ss -tulpn | grep 8080

# Gracefully terminate process by PID
sudo kill -15 <PID>
\`\`\`
**Tip**: Use \`kill -9\` only if standard SIGTERM does not terminate within 10 seconds.`;
  }

  if (p.includes("find") || p.includes("large") || p.includes("disk") || p.includes("space")) {
    return `### 💾 Disk Space & Large File Finder
\`\`\`bash
# Find top 10 largest directories in /
sudo du -ahx / | sort -rh | head -n 10

# Find files larger than 100MB
sudo find / -type f -size +100M -exec ls -lh {} \\; 2>/dev/null | awk '{ print $9 ": " $5 }'
\`\`\`
**Explanation**: Quickly frees up clogged server storage partitions.`;
  }

  if (p.includes("nginx") || p.includes("ssl") || p.includes("cert")) {
    return `### 🌐 Nginx & SSL Operations
\`\`\`bash
# Test Nginx configuration syntax before reloading
sudo nginx -t && sudo systemctl reload nginx

# Check SSL certificate expiration date
echo | openssl s_client -servername example.com -connect example.com:443 2>/dev/null | openssl x509 -noout -dates
\`\`\``;
  }

  return `### ⚡ Recommended Shell Command
\`\`\`bash
# Execute requested operation
${prompt.trim().startsWith("sudo") ? prompt : `sudo ${prompt}`}
\`\`\`
**Explanation**: Standard POSIX-compliant bash command. Ensure you have appropriate root/sudo privileges before execution.`;
}

startServer();
