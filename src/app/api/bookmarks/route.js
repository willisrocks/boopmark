import sql from "@/app/api/utils/sql";
import { auth } from "@/auth";

export async function GET(request) {
  try {
    const session = await auth();

    if (!session?.user?.id) {
      return Response.json(
        { error: "Authentication required" },
        { status: 401 },
      );
    }

    const { searchParams } = new URL(request.url);
    const search = searchParams.get("search");
    const tags = searchParams.get("tags");
    const sortBy = searchParams.get("sortBy") || "created_at";
    const sortOrder = searchParams.get("sortOrder") || "desc";

    let query = `
      SELECT id, url, title, description, image_url, domain, tags, created_at, updated_at
      FROM bookmarks
      WHERE user_id = $1
    `;
    const params = [session.user.id];
    let paramCount = 1;

    // Add search filter
    if (search && search.trim()) {
      paramCount++;
      query += ` AND (
        to_tsvector('english', COALESCE(title, '') || ' ' || COALESCE(description, '') || ' ' || COALESCE(url, '')) 
        @@ plainto_tsquery('english', $${paramCount})
      )`;
      params.push(search.trim());
    }

    // Add tags filter
    if (tags && tags.trim()) {
      const tagArray = tags
        .split(",")
        .map((tag) => tag.trim())
        .filter(Boolean);
      if (tagArray.length > 0) {
        paramCount++;
        query += ` AND tags && $${paramCount}`;
        params.push(tagArray);
      }
    }

    // Add sorting
    const validSortColumns = ["created_at", "title", "domain"];
    const validSortOrders = ["asc", "desc"];

    if (
      validSortColumns.includes(sortBy) &&
      validSortOrders.includes(sortOrder)
    ) {
      query += ` ORDER BY ${sortBy} ${sortOrder.toUpperCase()}`;
    } else {
      query += ` ORDER BY created_at DESC`;
    }

    const bookmarks = await sql(query, params);

    return Response.json(bookmarks);
  } catch (error) {
    console.error("Error fetching bookmarks:", error);
    return Response.json(
      { error: "Failed to fetch bookmarks" },
      { status: 500 },
    );
  }
}

export async function POST(request) {
  try {
    const session = await auth();

    if (!session?.user?.id) {
      return Response.json(
        { error: "Authentication required" },
        { status: 401 },
      );
    }

    const body = await request.json();
    const { url, title, description, image_url, domain, tags } = body;

    if (!url) {
      return Response.json({ error: "URL is required" }, { status: 400 });
    }

    const result = await sql`
      INSERT INTO bookmarks (url, title, description, image_url, domain, tags, user_id)
      VALUES (${url}, ${title || null}, ${description || null}, ${image_url || null}, ${domain || null}, ${tags || []}, ${session.user.id})
      RETURNING *
    `;

    return Response.json(result[0]);
  } catch (error) {
    console.error("Error creating bookmark:", error);
    return Response.json(
      { error: "Failed to create bookmark" },
      { status: 500 },
    );
  }
}
