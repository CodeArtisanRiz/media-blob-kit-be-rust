UPDATE projects p
SET storage_used_bytes = COALESCE((
    SELECT SUM(size)
    FROM files f
    WHERE f.project_id = p.id
), 0);
