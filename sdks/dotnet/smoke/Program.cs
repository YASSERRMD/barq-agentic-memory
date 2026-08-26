using BarqMemory;

var base_url = Environment.GetEnvironmentVariable("BARQ_BASE") ?? "http://127.0.0.1:18099";
using var client = new MemoryClient(base_url);

var saved = await client.RememberAsync(".NET SDK smoke fact", tenantId: "acme");
if (string.IsNullOrEmpty(saved.Id)) throw new Exception("remember failed");

var hits = await client.RecallAsync("sdk smoke fact", "acme", 5);
if (!hits.Any(h => h.Id == saved.Id)) throw new Exception("recall failed");

var successor = await client.UpdateAsync(saved.Id, ".NET SDK smoke fact v2");
var chain = await client.HistoryAsync(successor.Id);
if (chain.Length != 2) throw new Exception($"history {chain.Length} != 2");

await client.ForgetAsync(successor.Id);
Console.WriteLine("DOTNET SDK SMOKE TEST OK");
