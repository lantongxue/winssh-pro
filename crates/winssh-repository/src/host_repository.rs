//! `host_groups` / `hosts` 两个表的纯数据访问。

use std::collections::HashMap;

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, ExprTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, sea_query::Expr,
};

use winssh_db::entities::{host, host_group};

/// 主机与分组的数据操作。无状态，廉价克隆。
#[derive(Clone, Default)]
pub struct HostRepository;

impl HostRepository {
    /// 全部分组与各组主机，按用户排出来的顺序返回：
    /// 分组按建组先后，组内主机按列表里的顺序。空分组也返回。
    pub async fn load_groups(
        &self,
        conn: &impl ConnectionTrait,
    ) -> Result<Vec<(host_group::Model, Vec<host::Model>)>, DbErr> {
        let groups = host_group::Entity::find()
            .order_by_asc(host_group::Column::SortOrder)
            .order_by_asc(host_group::Column::Id)
            .all(conn)
            .await?;
        let hosts = host::Entity::find()
            .order_by_asc(host::Column::GroupId)
            .order_by_asc(host::Column::SortOrder)
            .order_by_asc(host::Column::Id)
            .all(conn)
            .await?;

        let mut by_group: HashMap<i64, Vec<host::Model>> = HashMap::new();
        for host in hosts {
            by_group.entry(host.group_id).or_default().push(host);
        }
        Ok(groups
            .into_iter()
            .map(|group| {
                let hosts = by_group.remove(&group.id).unwrap_or_default();
                (group, hosts)
            })
            .collect())
    }

    /// 按名字找分组。分组名唯一，找不到就是没有。
    pub async fn find_group(
        &self,
        conn: &impl ConnectionTrait,
        name: &str,
    ) -> Result<Option<host_group::Model>, DbErr> {
        host_group::Entity::find()
            .filter(host_group::Column::Name.eq(name))
            .one(conn)
            .await
    }

    /// 现有分组里最大的排序号，没有分组时是 0。
    pub async fn max_group_sort_order(&self, conn: &impl ConnectionTrait) -> Result<i64, DbErr> {
        // 空表上的 MAX() 返回一行 NULL，所以解出来是两层 Option：
        // 外层是"有没有这一行"，内层是"这一行是不是 NULL"。
        let max: Option<Option<i64>> = host_group::Entity::find()
            .select_only()
            .column_as(
                Expr::col(host_group::Column::SortOrder).max(),
                "max_sort_order",
            )
            .into_tuple()
            .one(conn)
            .await?;
        Ok(max.flatten().unwrap_or(0))
    }

    /// 新建一个分组，返回带 id 的完整记录。
    pub async fn insert_group(
        &self,
        conn: &impl ConnectionTrait,
        name: &str,
        sort_order: i64,
    ) -> Result<host_group::Model, DbErr> {
        host_group::Entity::insert(host_group::ActiveModel {
            name: Set(name.to_owned()),
            sort_order: Set(sort_order),
            ..Default::default()
        })
        .exec(conn)
        .await?;
        // id 是自增的，插完按名字（唯一）取回，省去 RETURNING 的后端差异。
        self.find_group(conn, name)
            .await?
            .ok_or_else(|| DbErr::RecordNotFound(format!("host_groups {name}")))
    }

    /// 删除一个分组。
    pub async fn delete_group(
        &self,
        conn: &impl ConnectionTrait,
        group_id: i64,
    ) -> Result<(), DbErr> {
        host_group::Entity::delete_by_id(group_id)
            .exec(conn)
            .await?;
        Ok(())
    }

    /// 某个分组下有多少台主机。
    pub async fn count_hosts_in_group(
        &self,
        conn: &impl ConnectionTrait,
        group_id: i64,
    ) -> Result<u64, DbErr> {
        host::Entity::find()
            .filter(host::Column::GroupId.eq(group_id))
            .count(conn)
            .await
    }

    /// 组内最大的排序号，空组是 0。
    pub async fn max_host_sort_in_group(
        &self,
        conn: &impl ConnectionTrait,
        group_id: i64,
    ) -> Result<i64, DbErr> {
        // 空组上的 MAX() 返回一行 NULL，两层 Option 的道理同 [`Self::max_group_sort_order`]。
        let max: Option<Option<i64>> = host::Entity::find()
            .filter(host::Column::GroupId.eq(group_id))
            .select_only()
            .column_as(Expr::col(host::Column::SortOrder).max(), "max_sort_order")
            .into_tuple()
            .one(conn)
            .await?;
        Ok(max.flatten().unwrap_or(0))
    }

    /// 按 id 找一台主机。
    pub async fn find_host(
        &self,
        conn: &impl ConnectionTrait,
        id: &str,
    ) -> Result<Option<host::Model>, DbErr> {
        host::Entity::find_by_id(id).one(conn).await
    }

    /// 插入一台主机，主键（id）必须已经填好。
    pub async fn insert_host(
        &self,
        conn: &impl ConnectionTrait,
        host: host::ActiveModel,
    ) -> Result<(), DbErr> {
        host::Entity::insert(host).exec(conn).await?;
        Ok(())
    }

    /// 按主键更新一台主机的配置字段；只有 `Set` 了的字段会被写。
    /// 主机不存在时返回 [`DbErr::RecordNotFound`]。
    pub async fn update_host(
        &self,
        conn: &impl ConnectionTrait,
        host: host::ActiveModel,
    ) -> Result<(), DbErr> {
        host.update(conn).await?;
        Ok(())
    }

    /// 删除一台主机，返回是否真的删掉了一行。
    pub async fn delete_host(&self, conn: &impl ConnectionTrait, id: &str) -> Result<bool, DbErr> {
        let result = host::Entity::delete_by_id(id).exec(conn).await?;
        Ok(result.rows_affected == 1)
    }

    /// 把组内排在 `after` 之后的主机整体往后挪一位，腾出一个空位。
    pub async fn shift_host_sort_orders(
        &self,
        conn: &impl ConnectionTrait,
        group_id: i64,
        after: i64,
    ) -> Result<(), DbErr> {
        host::Entity::update_many()
            .col_expr(
                host::Column::SortOrder,
                Expr::col(host::Column::SortOrder).add(1),
            )
            .filter(host::Column::GroupId.eq(group_id))
            .filter(host::Column::SortOrder.gt(after))
            .exec(conn)
            .await?;
        Ok(())
    }

    /// 只改一台主机的采集间隔。
    ///
    /// 走 `update_many` 而不是拼一个 `ActiveModel`：那条路要把整行的列都摆出来，
    /// 而调用方（监控面板里的设置页）手上只有这两个数，别的列它既不知道也不该动。
    pub async fn update_host_intervals(
        &self,
        conn: &impl ConnectionTrait,
        id: &str,
        monitor_secs: u32,
        status_secs: u32,
    ) -> Result<u64, DbErr> {
        let result = host::Entity::update_many()
            .col_expr(
                host::Column::MonitorIntervalSecs,
                Expr::value(monitor_secs as i64),
            )
            .col_expr(
                host::Column::StatusIntervalSecs,
                Expr::value(status_secs as i64),
            )
            .filter(host::Column::Id.eq(id))
            .exec(conn)
            .await?;
        Ok(result.rows_affected)
    }

    /// 只改一台主机的端口转发规则那一列。
    ///
    /// 与 [`Self::update_host_intervals`] 同一个道理：调用方（会话里的端口转发
    /// 对话框）手上只有这一串规则，别的列它既不知道也不该动。
    pub async fn update_host_forwards(
        &self,
        conn: &impl ConnectionTrait,
        id: &str,
        forwards: &str,
    ) -> Result<u64, DbErr> {
        let result = host::Entity::update_many()
            .col_expr(host::Column::Forwards, Expr::value(forwards))
            .filter(host::Column::Id.eq(id))
            .exec(conn)
            .await?;
        Ok(result.rows_affected)
    }

    /// 只改一台主机的 shell 集成开关那一列。
    ///
    /// 与上面两条同一个道理：调用方（命令历史那扇窗）手上只有这一个布尔值。
    pub async fn update_host_shell_integration(
        &self,
        conn: &impl ConnectionTrait,
        id: &str,
        enabled: bool,
    ) -> Result<u64, DbErr> {
        let result = host::Entity::update_many()
            .col_expr(host::Column::ShellIntegration, Expr::value(enabled))
            .filter(host::Column::Id.eq(id))
            .exec(conn)
            .await?;
        Ok(result.rows_affected)
    }

    /// 改一台主机的落位：换到哪个分组、排在第几位。
    pub async fn update_host_position(
        &self,
        conn: &impl ConnectionTrait,
        id: &str,
        group_id: i64,
        sort_order: i64,
    ) -> Result<(), DbErr> {
        host::Entity::update_many()
            .col_expr(host::Column::GroupId, Expr::value(group_id))
            .col_expr(host::Column::SortOrder, Expr::value(sort_order))
            .filter(host::Column::Id.eq(id))
            .exec(conn)
            .await?;
        Ok(())
    }
}
