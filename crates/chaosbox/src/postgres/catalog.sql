BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout = '30s';
SET LOCAL lock_timeout = '5s';
SET LOCAL search_path = pg_catalog;
WITH selected_schema AS (
  SELECT oid, nspname FROM pg_namespace WHERE nspname = :'schema'
), relations AS (
  SELECT c.*, n.nspname FROM pg_class c JOIN selected_schema n ON n.oid = c.relnamespace
  WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f')
), records AS (
  SELECT 'relation:' || c.oid AS key,
    CASE c.relkind WHEN 'v' THEN 'view' WHEN 'm' THEN 'materialized_view' WHEN 'f' THEN 'foreign_table' ELSE 'table' END AS kind,
    c.nspname AS schema, c.relname AS name, NULL::text AS parent, NULL::text AS target,
    CASE WHEN c.relkind IN ('v', 'm') THEN pg_get_viewdef(c.oid, false) END AS definition,
    jsonb_build_object('oid', c.oid, 'relkind', c.relkind, 'select_privilege', has_table_privilege(c.oid, 'SELECT'),
      'comment', obj_description(c.oid, 'pg_class')) AS details
  FROM relations c
  UNION ALL
  SELECT 'column:' || c.oid || ':' || a.attnum, 'column', c.nspname, a.attname,
    'relation:' || c.oid, NULL, NULL,
    jsonb_build_object('position', a.attnum, 'type', format_type(a.atttypid, a.atttypmod),
      'nullable', NOT a.attnotnull, 'default', pg_get_expr(d.adbin, d.adrelid, false),
      'identity', a.attidentity, 'generated', a.attgenerated, 'comment', col_description(c.oid, a.attnum))
  FROM relations c JOIN pg_attribute a ON a.attrelid = c.oid
  LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
  WHERE a.attnum > 0 AND NOT a.attisdropped
  UNION ALL
  SELECT 'constraint:' || k.oid, 'constraint', c.nspname, k.conname, 'relation:' || c.oid,
    CASE WHEN k.contype = 'f' THEN 'relation:' || k.confrelid END,
    pg_get_constraintdef(k.oid, false),
    jsonb_build_object('type', k.contype, 'validated', k.convalidated, 'columns', k.conkey,
      'target_columns', k.confkey, 'target_schema', tn.nspname, 'target_name', tc.relname,
      'update_action', k.confupdtype, 'delete_action', k.confdeltype, 'match_type', k.confmatchtype)
  FROM pg_constraint k JOIN relations c ON k.conrelid = c.oid
  LEFT JOIN pg_class tc ON tc.oid = k.confrelid LEFT JOIN pg_namespace tn ON tn.oid = tc.relnamespace
  UNION ALL
  SELECT 'index:' || i.indexrelid, 'index', c.nspname, ic.relname, 'relation:' || c.oid, NULL,
    pg_get_indexdef(i.indexrelid, 0, false), jsonb_build_object('unique', i.indisunique, 'primary', i.indisprimary, 'valid', i.indisvalid)
  FROM pg_index i JOIN relations c ON c.oid = i.indrelid JOIN pg_class ic ON ic.oid = i.indexrelid
  UNION ALL
  SELECT 'routine:' || p.oid, 'routine', n.nspname, p.proname, NULL, NULL,
    CASE WHEN p.prokind IN ('f', 'p') THEN pg_get_functiondef(p.oid) END,
    jsonb_build_object('kind', p.prokind, 'arguments', pg_get_function_identity_arguments(p.oid),
      'returns', pg_get_function_result(p.oid), 'definition_omitted', p.prokind NOT IN ('f', 'p'))
  FROM pg_proc p JOIN selected_schema n ON p.pronamespace = n.oid
)
SELECT jsonb_build_object('database', current_database(), 'schema', :'schema',
  'role', current_user, 'server_version', current_setting('server_version_num'),
  'read_only', current_setting('transaction_read_only'), 'schema_exists', EXISTS(SELECT 1 FROM selected_schema),
  'schema_usage', COALESCE((SELECT has_schema_privilege(oid, 'USAGE') FROM selected_schema), false),
  'records', COALESCE((SELECT jsonb_agg(to_jsonb(records) ORDER BY key COLLATE "C") FROM records), '[]'::jsonb),
  'omissions', jsonb_build_object('row_data', 'not collected', 'triggers_sequences_types_policies', 'not collected',
    'cross_schema_targets', 'recorded on constraint; no endpoint outside the selected schema',
    'routine_definitions', COALESCE((SELECT count(*) FROM records WHERE kind = 'routine' AND definition IS NULL), 0),
    'tables_without_select', COALESCE((SELECT count(*) FROM records WHERE kind IN ('table','view','materialized_view','foreign_table') AND details->>'select_privilege' = 'false'), 0)));
COMMIT;
