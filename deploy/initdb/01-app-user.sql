-- 应用用户：与  的 db.url 保持一致（postgres://cicada:...@localhost:5432/cicada）。
CREATE ROLE cicada LOGIN PASSWORD '2026@cicada';
GRANT ALL PRIVILEGES ON DATABASE cicada TO cicada;
GRANT ALL ON SCHEMA public TO cicada;
