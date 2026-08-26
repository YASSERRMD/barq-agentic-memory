// Barq memory engine client for .NET — mirrors the Rust/Python/TypeScript
// SDKs concept-for-concept. System.Text.Json + HttpClient, no packages.
using System.Net.Http.Json;
using System.Text.Json.Serialization;

namespace BarqMemory;

/// <summary>API-level failure (status, message).</summary>
public sealed class BarqException : Exception
{
    public int Status { get; }
    public BarqException(int status, string message) : base($"api ({status}): {message}")
        => Status = status;
}

/// <summary>A canonical memory as seen on the wire.</summary>
public sealed record MemoryView
{
    [JsonPropertyName("id")] public string Id { get; init; } = "";
    [JsonPropertyName("type")] public string Type { get; init; } = "semantic";
    [JsonPropertyName("text")] public string Text { get; init; } = "";
    [JsonPropertyName("status")] public string Status { get; init; } = "active";
    [JsonPropertyName("version")] public ulong Version { get; init; }
    [JsonPropertyName("confidence")] public float Confidence { get; init; }
    [JsonPropertyName("created_at")] public string CreatedAt { get; init; } = "";
    [JsonPropertyName("updated_at")] public string UpdatedAt { get; init; } = "";
}

/// <summary>Recall hit with its similarity score.</summary>
public sealed record ScoredMemory
{
    [JsonPropertyName("id")] public string Id { get; init; } = "";
    [JsonPropertyName("type")] public string Type { get; init; } = "semantic";
    [JsonPropertyName("text")] public string Text { get; init; } = "";
    [JsonPropertyName("score")] public float Score { get; init; }
}

/// <summary>Client for one Barq memory server.</summary>
public sealed class MemoryClient : IDisposable
{
    private readonly HttpClient _http;
    private readonly string _base;

    public MemoryClient(string baseUrl)
    {
        _base = baseUrl.TrimEnd('/');
        _http = new HttpClient { BaseAddress = new Uri(_base) };
    }

    private async Task<T> SendAsync<T>(HttpMethod method, string path, object? body = null)
        where T : class
    {
        using var request = new HttpRequestMessage(method, path);
        if (body is not null)
            request.Content = JsonContent.Create(body);
        using var response = await _http.SendAsync(request);
        var text = await response.Content.ReadAsStringAsync();
        if (!response.IsSuccessStatusCode)
        {
            string message = text;
            try
            {
                using var doc = System.Text.Json.JsonDocument.Parse(text);
                if (doc.RootElement.TryGetProperty("message", out var m))
                    message = m.GetString() ?? text;
            }
            catch (System.Text.Json.JsonException) { }
            throw new BarqException((int)response.StatusCode, message);
        }
        if (string.IsNullOrEmpty(text)) return (T)(object)null!;
        return System.Text.Json.JsonSerializer.Deserialize<T>(text)
               ?? throw new BarqException(0, "empty body");
    }

    public Task<MemoryView> RememberAsync(string text, string? tenantId = null,
        string? userId = null, string? type = null, float? confidence = null)
        => SendAsync<MemoryView>(HttpMethod.Post, "/v1/memories", new
        {
            text,
            tenant_id = tenantId,
            user_id = userId,
            type,
            confidence,
        });

    public Task<MemoryView[]> SearchAsync(string query, string? tenantId = null, int limit = 10)
        => SendAsync<MemoryView[]>(HttpMethod.Post, "/v1/search", new
        {
            query, tenant_id = tenantId, limit,
        });

    public Task<ScoredMemory[]> RecallAsync(string query, string? tenantId = null, int limit = 10)
        => SendAsync<ScoredMemory[]>(HttpMethod.Post, "/v1/recall", new
        {
            query, tenant_id = tenantId, limit,
        });

    public Task<MemoryView> UpdateAsync(string id, string newText)
        => SendAsync<MemoryView>(HttpMethod.Patch, $"/v1/memories/{id}", new { text = newText });

    public async Task ForgetAsync(string id, bool hard = false)
    {
        var suffix = hard ? "?hard=true" : "";
        await SendAsync<object>(HttpMethod.Delete, $"/v1/memories/{id}{suffix}");
    }

    public Task<MemoryView[]> HistoryAsync(string id)
        => SendAsync<MemoryView[]>(HttpMethod.Get, $"/v1/memories/{id}/history");

    public void Dispose() => _http.Dispose();
}
