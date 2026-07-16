use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m_009_create_oauth"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // OAuth client registrations (dynamic client registration)
        manager
            .create_table(
                Table::create()
                    .table(OauthClients::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OauthClients::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(OauthClients::ClientId)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(OauthClients::ClientSecret).string().null())
                    .col(ColumnDef::new(OauthClients::ClientName).string().null())
                    .col(ColumnDef::new(OauthClients::RedirectUris).json().not_null())
                    .col(
                        ColumnDef::new(OauthClients::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        // Authorization codes (short-lived, exchanged for tokens)
        manager
            .create_table(
                Table::create()
                    .table(OauthCodes::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OauthCodes::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(OauthCodes::Code)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(OauthCodes::ClientId).string().not_null())
                    .col(ColumnDef::new(OauthCodes::UserId).integer().not_null())
                    .col(ColumnDef::new(OauthCodes::RedirectUri).string().not_null())
                    .col(
                        ColumnDef::new(OauthCodes::CodeChallenge)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthCodes::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthCodes::Used)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(OauthCodes::Table, OauthCodes::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // OAuth tokens (access + refresh)
        manager
            .create_table(
                Table::create()
                    .table(OauthTokens::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OauthTokens::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(OauthTokens::AccessToken)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(OauthTokens::RefreshToken)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(OauthTokens::ClientId).string().not_null())
                    .col(ColumnDef::new(OauthTokens::UserId).integer().not_null())
                    .col(
                        ColumnDef::new(OauthTokens::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthTokens::Revoked)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(OauthTokens::Table, OauthTokens::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(OauthTokens::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(OauthCodes::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(OauthClients::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum OauthClients {
    Table,
    Id,
    ClientId,
    ClientSecret,
    ClientName,
    RedirectUris,
    CreatedAt,
}

#[derive(Iden)]
enum OauthCodes {
    Table,
    Id,
    Code,
    ClientId,
    UserId,
    RedirectUri,
    CodeChallenge,
    ExpiresAt,
    Used,
}

#[derive(Iden)]
enum OauthTokens {
    Table,
    Id,
    AccessToken,
    RefreshToken,
    ClientId,
    UserId,
    ExpiresAt,
    Revoked,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}
