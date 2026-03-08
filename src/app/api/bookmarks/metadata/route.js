export async function POST(request) {
  try {
    const body = await request.json();
    const { url } = body;

    if (!url) {
      return Response.json({ error: "URL is required" }, { status: 400 });
    }

    // Fetch the webpage content
    const response = await fetch(url, {
      headers: {
        "User-Agent": "Mozilla/5.0 (compatible; BookmarkBot/1.0)",
      },
    });

    if (!response.ok) {
      throw new Error(`HTTP ${response.status}: ${response.statusText}`);
    }

    const html = await response.text();

    // Extract domain
    const urlObj = new URL(url);
    const domain = urlObj.hostname;

    // Simple regex-based metadata extraction
    const titleMatch = html.match(/<title[^>]*>([^<]*)<\/title>/i);
    const descriptionMatch =
      html.match(
        /<meta[^>]*name=["']description["'][^>]*content=["']([^"']*)["'][^>]*>/i,
      ) ||
      html.match(
        /<meta[^>]*property=["']og:description["'][^>]*content=["']([^"']*)["'][^>]*>/i,
      );
    const imageMatch =
      html.match(
        /<meta[^>]*property=["']og:image["'][^>]*content=["']([^"']*)["'][^>]*>/i,
      ) ||
      html.match(/<link[^>]*rel=["']icon["'][^>]*href=["']([^"']*)["'][^>]*>/i);

    let title = titleMatch ? titleMatch[1].trim() : "";
    let description = descriptionMatch ? descriptionMatch[1].trim() : "";
    let image_url = imageMatch ? imageMatch[1].trim() : "";

    // Make relative URLs absolute
    if (image_url && !image_url.startsWith("http")) {
      try {
        image_url = new URL(image_url, url).href;
      } catch (e) {
        image_url = "";
      }
    }

    // Clean up title and description
    title = title.replace(/\s+/g, " ").trim();
    description = description.replace(/\s+/g, " ").trim();

    return Response.json({
      title: title || domain,
      description: description || "",
      image_url: image_url || "",
      domain,
    });
  } catch (error) {
    console.error("Error fetching metadata:", error);

    // Return basic info if metadata extraction fails
    try {
      const body = await request.json();
      const urlObj = new URL(body.url);
      return Response.json({
        title: urlObj.hostname,
        description: "",
        image_url: "",
        domain: urlObj.hostname,
      });
    } catch (urlError) {
      return Response.json({ error: "Invalid URL" }, { status: 400 });
    }
  }
}
